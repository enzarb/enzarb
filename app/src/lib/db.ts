import postgres from 'postgres';
import { DEFAULT_ROLES } from './privileges';

let _sql: ReturnType<typeof postgres> | null = null;

export function getDb() {
	if (_sql) return _sql;
	const url = process.env.DATABASE_URL ?? 'postgres://localhost/enzarb';
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

	for (const [key, value] of Object.entries(defaultSettings)) {
		await sql`
			INSERT INTO app_settings (key, value) VALUES (${key}, ${value})
			ON CONFLICT (key) DO NOTHING
		`;
	}
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
	pricing_cpu_seconds_per_unit: '0.0000139',
	pricing_mem_gib_seconds_per_unit: '0.0000028',
	pricing_net_ingress_per_gib: '0.1073741824',
	pricing_net_egress_per_gib: '0.9663676416',
	pricing_storage_gib_seconds_per_unit: '0.0000000385',
	pricing_zot_storage_gib_seconds_per_unit: '0.0000000385',
	pricing_free_cpu_seconds: '36000',
	pricing_free_mem_gib_seconds: '107374182'
};
