-- app schema: users and sessions. Owned and migrated by enzarb-ui.
CREATE TABLE app.users (
	id          bigserial PRIMARY KEY,
	github_id   bigint      NOT NULL UNIQUE,
	login       text        NOT NULL,
	name        text,
	avatar_url  text,
	created_at  timestamptz NOT NULL DEFAULT now(),
	updated_at  timestamptz NOT NULL DEFAULT now()
);

-- id is the hex sha-256 of the session token; the token itself is never stored.
CREATE TABLE app.sessions (
	id          text        PRIMARY KEY,
	user_id     bigint      NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
	expires_at  timestamptz NOT NULL,
	created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sessions_user_id_idx ON app.sessions (user_id);
CREATE INDEX sessions_expires_at_idx ON app.sessions (expires_at);

-- Runtime role (pgop name enzarb_ui) gets data access only. Skipped where the role does not exist (dev, tests).
DO $$
BEGIN
	IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
		GRANT USAGE ON SCHEMA app TO enzarb_ui;
		GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA app TO enzarb_ui;
		GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA app TO enzarb_ui;
		ALTER DEFAULT PRIVILEGES IN SCHEMA app GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO enzarb_ui;
		ALTER DEFAULT PRIVILEGES IN SCHEMA app GRANT USAGE, SELECT ON SEQUENCES TO enzarb_ui;
	END IF;
END
$$;
