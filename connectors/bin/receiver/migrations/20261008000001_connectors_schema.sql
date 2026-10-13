-- Schema `connectors` (owner: enzarb_connectors_owner). History lives in connectors._sqlx_migrations.
-- pgop creates this schema. Plain `CREATE SCHEMA IF NOT EXISTS` needs database-level CREATE even when
-- the schema exists, which this role does not have, so only create it when it is missing.
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'connectors') THEN
        CREATE SCHEMA connectors;
    END IF;
END $$;

-- One row per GitHub App installation, maintained from installation and
-- installation_repositories webhooks. `repos` is a sorted JSON array of
-- lowercase repo keys (`github:owner/repo`).
CREATE TABLE connectors.installations (
    installation_id bigint PRIMARY KEY,
    account         text        NOT NULL,
    repos           jsonb       NOT NULL DEFAULT '[]'::jsonb,
    updated_at      timestamptz NOT NULL DEFAULT now()
);

-- Runtime role is created by pgop in prod; absent roles (bare local DBs) are skipped with a notice.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_connectors') THEN
        GRANT USAGE ON SCHEMA connectors TO enzarb_connectors;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA connectors TO enzarb_connectors;
        ALTER DEFAULT PRIVILEGES IN SCHEMA connectors
            GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO enzarb_connectors;
    ELSE
        RAISE NOTICE 'role enzarb_connectors missing; grants skipped';
    END IF;
END
$$;
