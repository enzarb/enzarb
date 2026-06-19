import postgres from 'postgres';

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
}
