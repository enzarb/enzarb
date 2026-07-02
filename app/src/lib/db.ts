import postgres from 'postgres';
import { env } from '$env/dynamic/private';
import { DEFAULT_ROLES } from './privileges';

let _sql: ReturnType<typeof postgres> | null = null;

export function getDb() {
	if (_sql) return _sql;
	const url = env.DATABASE_URL ?? 'postgres://localhost/enzarb';
	_sql = postgres(url, { max: 10, idle_timeout: 30, connect_timeout: 10 });
	return _sql;
}

// Lazy tagged template proxy — safe to import at build time
export const sql: ReturnType<typeof postgres> = new Proxy(
	function () {} as unknown as ReturnType<typeof postgres>,
	{
		apply(_target, _this, args) {
			return (getDb() as any)(...args);
		},
		get(_target, prop) {
			return (getDb() as any)[prop];
		}
	}
);


export async function migrate() {
	await sql`
		CREATE TABLE IF NOT EXISTS users (
			id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
			email TEXT NOT NULL UNIQUE,
			oidc_sub TEXT NOT NULL UNIQUE,
			is_admin BOOLEAN NOT NULL DEFAULT false,
			created_at TIMESTAMPTZ NOT NULL DEFAULT now()
		)
	`;
	await sql`
		CREATE TABLE IF NOT EXISTS organizations (
			id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
			slug TEXT NOT NULL UNIQUE,
			display_name TEXT NOT NULL,
			tier TEXT NOT NULL DEFAULT 'free',
			created_at TIMESTAMPTZ NOT NULL DEFAULT now()
		)
	`;
	await sql`
		CREATE TABLE IF NOT EXISTS org_members (
			org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
			user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
			role TEXT NOT NULL DEFAULT 'member',
			PRIMARY KEY (org_id, user_id)
		)
	`;
	await sql`
		CREATE TABLE IF NOT EXISTS sessions (
			id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
			user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
			data JSONB NOT NULL DEFAULT '{}',
			created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
			expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '7 days'
		)
	`;
	// Privilege-based roles: per-org, editable bags of privileges. org_members.role
	// references org_roles.name. Seeded with builtin owner/manager/member below.
	await sql`
		CREATE TABLE IF NOT EXISTS org_roles (
			org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
			name TEXT NOT NULL,
			privileges TEXT[] NOT NULL DEFAULT '{}',
			builtin BOOLEAN NOT NULL DEFAULT false,
			PRIMARY KEY (org_id, name)
		)
	`;
	// Back-compat: the legacy two-role model used 'admin'; map it to the new owner role.
	await sql`UPDATE org_members SET role = 'owner' WHERE role = 'admin'`;
	// Seed builtin roles into every org that lacks them (new + pre-existing).
	const allOrgs = await sql`SELECT id FROM organizations`;
	for (const { id } of allOrgs) {
		await seedOrgRoles(id);
	}
	await sql`CREATE INDEX IF NOT EXISTS sessions_user_id ON sessions(user_id)`;
	await sql`CREATE INDEX IF NOT EXISTS sessions_expires_at ON sessions(expires_at)`;
	await sql`ALTER TABLE users ADD COLUMN IF NOT EXISTS username TEXT UNIQUE`;
	await sql`
		CREATE TABLE IF NOT EXISTS usage_events (
			id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
			org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
			project_id TEXT NOT NULL,
			resource_type TEXT NOT NULL,
			quantity NUMERIC NOT NULL,
			unit TEXT NOT NULL,
			recorded_at TIMESTAMPTZ NOT NULL DEFAULT now()
		)
	`;
	await sql`CREATE INDEX IF NOT EXISTS usage_events_org_period ON usage_events(org_id, recorded_at)`;
	// Component dimension: distinguishes where a usage event was incurred so the
	// dashboard can split workspace vs deploy-environment spend and surface
	// registry usage. Values: 'workspace' | 'environment' | 'zot'.
	await sql`ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS component TEXT NOT NULL DEFAULT 'workspace'`;
	// Deploy-environment slug for component='environment' rows; NULL otherwise.
	await sql`ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS environment TEXT`;
	await sql`CREATE INDEX IF NOT EXISTS usage_events_org_comp_period ON usage_events(org_id, component, recorded_at)`;
	// Fine-grained label for per-resource drill-down: pod name for compute, PVC
	// name for block storage, image path for registry. NULL for network events.
	await sql`ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS label TEXT`;
	// K8s owner of the label resource ("Deployment/my-app", "StatefulSet/db").
	// Used to group pods by workload controller in the project billing view.
	await sql`ALTER TABLE usage_events ADD COLUMN IF NOT EXISTS owner TEXT`;
	// Soft-delete marker for organizations; non-null = within the retention
	// window (recoverable until the operator purges the Organization CR).
	await sql`ALTER TABLE organizations ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ`;
	await sql`
		CREATE TABLE IF NOT EXISTS invoices (
			id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
			org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
			period_start TIMESTAMPTZ NOT NULL,
			period_end TIMESTAMPTZ NOT NULL,
			total_cents BIGINT NOT NULL DEFAULT 0,
			status TEXT NOT NULL DEFAULT 'draft',
			created_at TIMESTAMPTZ NOT NULL DEFAULT now()
		)
	`;
	// Admin-editable platform settings, stored as a key/value table and read by
	// both the app and the billing worker. Seed defaults without clobbering
	// any operator-edited values.
	// Per-metric breakdown of an invoice, frozen at the rates in effect when the
	// billing worker generated it — unlike the live estimate (which always uses
	// current app_settings pricing), these amounts never change after insert,
	// so the invoice PDF stays accurate even if pricing is edited later.
	await sql`
		CREATE TABLE IF NOT EXISTS invoice_line_items (
			id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
			invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
			resource_type TEXT NOT NULL,
			quantity NUMERIC NOT NULL,
			unit TEXT NOT NULL,
			unit_price_cents NUMERIC NOT NULL,
			amount_cents BIGINT NOT NULL
		)
	`;
	await sql`CREATE INDEX IF NOT EXISTS invoice_line_items_invoice_id ON invoice_line_items(invoice_id)`;
	await sql`
		CREATE TABLE IF NOT EXISTS app_settings (
			key TEXT PRIMARY KEY,
			value TEXT NOT NULL,
			updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
		)
	`;
	// Network pricing moved from $/byte to $/GiB. Migrate any operator-customized
	// per-byte values to the new per-GiB keys (1 GiB = 1073741824 bytes) before
	// seeding defaults, then drop the old keys. Runs before the seed loop so the
	// converted value wins over the default (seed is ON CONFLICT DO NOTHING).
	for (const dir of ['ingress', 'egress']) {
		await sql`
			INSERT INTO app_settings (key, value)
			SELECT ${'pricing_net_' + dir + '_per_gib'}, (value::numeric * 1073741824)::text
			FROM app_settings WHERE key = ${'pricing_net_' + dir + '_per_byte'}
			ON CONFLICT (key) DO NOTHING
		`;
	}
	await sql`DELETE FROM app_settings WHERE key IN ('pricing_net_ingress_per_byte', 'pricing_net_egress_per_byte')`;

	// Network pricing split from a single ingress/egress rate into independent
	// internal vs external line items (each separately metered and billed).
	// Carry any operator-customized single rate over to the external key (the
	// rate that was effectively in force), then drop the old keys. Internal
	// rates seed from defaults below. Runs before the seed loop so migrated
	// values win over defaults (seed is ON CONFLICT DO NOTHING).
	for (const dir of ['ingress', 'egress']) {
		await sql`
			INSERT INTO app_settings (key, value)
			SELECT ${'pricing_net_' + dir + '_external_per_gib'}, value
			FROM app_settings WHERE key = ${'pricing_net_' + dir + '_per_gib'}
			ON CONFLICT (key) DO NOTHING
		`;
	}
	await sql`DELETE FROM app_settings WHERE key IN ('pricing_net_ingress_per_gib', 'pricing_net_egress_per_gib')`;

	for (const [key, value] of Object.entries(defaultSettings)) {
		await sql`
			INSERT INTO app_settings (key, value) VALUES (${key}, ${value})
			ON CONFLICT (key) DO NOTHING
		`;
	}

	// Billing unit migration: rename resource_type values in usage_events and
	// pricing keys in app_settings from legacy GiB-seconds/cpu-seconds to
	// standard vCPU-hours, GiB-hours, and GiB-months.
	//
	// usage_events: UPDATE rows under old resource_type names to the new ones.
	// Conversion factors per row (each row accumulated one 60-second tick):
	//   cpu_seconds         → vcpu_hours           ÷ 3600
	//   mem_gib_seconds     → mem_gib_hours         ÷ 3600
	//   storage_gib_seconds → block_storage_gib_months ÷ 2592000 (30d in seconds)
	//   zot_storage_gib_seconds → registry_gib_months  ÷ 2592000
	await sql`
		UPDATE usage_events SET resource_type = 'vcpu_hours', unit = 'vCPU-hr',
		  quantity = quantity / 3600
		WHERE resource_type = 'cpu_seconds'
	`;
	await sql`
		UPDATE usage_events SET resource_type = 'mem_gib_hours', unit = 'GiB-hr',
		  quantity = quantity / 3600
		WHERE resource_type = 'mem_gib_seconds'
	`;
	await sql`
		UPDATE usage_events SET resource_type = 'block_storage_gib_months', unit = 'GiB-mo',
		  quantity = quantity / 2592000
		WHERE resource_type = 'storage_gib_seconds'
	`;
	await sql`
		UPDATE usage_events SET resource_type = 'registry_gib_months', unit = 'GiB-mo',
		  quantity = quantity / 2592000
		WHERE resource_type = 'zot_storage_gib_seconds'
	`;

	// app_settings: migrate pricing keys, converting rates to the new units so
	// the dollar cost of one unit stays the same.
	//   cpu rate: old $/cpu-second × 3600 = new $/vCPU-hour
	//   mem rate: old $/GiB-second × 3600 = new $/GiB-hour
	//   storage rate: old $/GiB-second × 2592000 = new $/GiB-month
	for (const [oldKey, newKey, factor] of [
		['pricing_cpu_seconds_per_unit',          'pricing_vcpu_hours_per_unit',                3600],
		['pricing_mem_gib_seconds_per_unit',       'pricing_mem_gib_hours_per_unit',             3600],
		['pricing_storage_gib_seconds_per_unit',   'pricing_block_storage_gib_months_per_unit',  2592000],
		['pricing_zot_storage_gib_seconds_per_unit','pricing_registry_gib_months_per_unit',      2592000],
		['pricing_free_cpu_seconds',               'pricing_free_vcpu_hours',                    1 / 3600],
		['pricing_free_mem_gib_seconds',           'pricing_free_mem_gib_hours',                 1 / 3600],
		['pricing_free_storage_gib_seconds',       'pricing_free_block_storage_gib_months',      1 / 2592000],
		['pricing_free_zot_storage_gib_seconds',   'pricing_free_registry_gib_months',           1 / 2592000],
	] as [string, string, number][]) {
		await sql`
			INSERT INTO app_settings (key, value)
			SELECT ${newKey}, (value::numeric * ${factor})::text
			FROM app_settings WHERE key = ${oldKey}
			ON CONFLICT (key) DO NOTHING
		`;
		await sql`DELETE FROM app_settings WHERE key = ${oldKey}`;
	}

	// User-level secret env vars (apply to all projects for a user).
	await sql`
		CREATE TABLE IF NOT EXISTS user_secrets (
			id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
			user_id TEXT NOT NULL,
			key TEXT NOT NULL,
			value TEXT NOT NULL,
			created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
			UNIQUE (user_id, key)
		)
	`;

	// Project-level secret env vars (override user-level for a specific project).
	await sql`
		CREATE TABLE IF NOT EXISTS project_secrets (
			id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
			project_id TEXT NOT NULL,
			key TEXT NOT NULL,
			value TEXT NOT NULL,
			created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
			UNIQUE (project_id, key)
		)
	`;

	// GitHub login support: oidc_sub is now optional (GitHub-first users have none).
	await sql`ALTER TABLE users ALTER COLUMN oidc_sub DROP NOT NULL`;
	await sql`ALTER TABLE users ADD COLUMN IF NOT EXISTS github_id TEXT UNIQUE`;

	// Error log for server and client exceptions (M8).
	await sql`
		CREATE TABLE IF NOT EXISTS error_logs (
			id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
			scope TEXT NOT NULL DEFAULT 'application',
			level TEXT NOT NULL DEFAULT 'error',
			message TEXT NOT NULL,
			stack TEXT,
			context JSONB NOT NULL DEFAULT '{}',
			user_id UUID REFERENCES users(id) ON DELETE SET NULL,
			ip_address TEXT,
			created_at TIMESTAMPTZ NOT NULL DEFAULT now()
		)
	`;
	await sql`CREATE INDEX IF NOT EXISTS error_logs_scope_created_idx ON error_logs(scope, created_at DESC)`;

	// JWT revocation list — allows early token invalidation (L1).
	await sql`
		CREATE TABLE IF NOT EXISTS jwt_revocations (
			jti TEXT PRIMARY KEY,
			revoked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
			expires_at TIMESTAMPTZ NOT NULL
		)
	`;

	// Truncate historical recorded_at to the minute so charts don't show
	// zero-gaps caused by sub-minute tick drift (metering now writes truncated
	// timestamps going forward).
	await sql`UPDATE usage_events SET recorded_at = date_trunc('minute', recorded_at) WHERE recorded_at != date_trunc('minute', recorded_at)`;

	// Pending account links — stores in-flight GitHub ↔ existing-account links (M2).
	await sql`
		CREATE TABLE IF NOT EXISTS pending_account_links (
			id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
			github_id TEXT NOT NULL,
			github_email TEXT NOT NULL,
			github_display_name TEXT NOT NULL,
			github_access_token TEXT NOT NULL,
			existing_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
			token TEXT NOT NULL UNIQUE,
			expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '10 minutes'
		)
	`;
}

// seedOrgRoles inserts the builtin roles for an org, leaving any already-present
// (possibly customized) role untouched. Safe to call repeatedly.
export async function seedOrgRoles(orgId: string): Promise<void> {
	for (const role of DEFAULT_ROLES) {
		await sql`
			INSERT INTO org_roles (org_id, name, privileges, builtin)
			VALUES (${orgId}, ${role.name}, ${role.privileges}, true)
			ON CONFLICT (org_id, name) DO NOTHING
		`;
	}
}

// Default platform settings, seeded on first migrate. Keys mirror SettingKey in
// settings.ts; the billing worker (billing/cmd/billing) relies on the same keys.
export const defaultSettings: Record<string, string> = {
	free_max_pvc_gi: '5',
	retention_days: '30',
	// Per-unit billing rates ($), one per metered resource type.
	pricing_vcpu_hours_per_unit: '0.05004',
	pricing_mem_gib_hours_per_unit: '0.01008',
	pricing_block_storage_gib_months_per_unit: '0.099792',
	pricing_registry_gib_months_per_unit: '0.099792',
	// Network is metered as four independent line items (internal vs external,
	// ingress vs egress). Internal rates default to 0; tune in admin settings.
	pricing_net_ingress_internal_per_gib: '0',
	pricing_net_egress_internal_per_gib: '0',
	pricing_net_ingress_external_per_gib: '0.1073741824',
	pricing_net_egress_external_per_gib: '0.9663676416',
	// Free-tier monthly allowances, one per billed metric. Compute/storage are
	// in the metric's native unit; network is in GiB.
	pricing_free_vcpu_hours: '10',
	pricing_free_mem_gib_hours: '29827',
	pricing_free_block_storage_gib_months: '5',
	pricing_free_registry_gib_months: '2',
	pricing_free_net_ingress_internal_gib: '100',
	pricing_free_net_egress_internal_gib: '100',
	pricing_free_net_ingress_external_gib: '10',
	pricing_free_net_egress_external_gib: '5'
};
