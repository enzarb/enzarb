-- Per-user Claude token, stored encrypted. owner is the GitHub login (no FK: the proxy and workflows
-- repos have no user ids). The UI never touches the table; it reads app_api.claude_credential_v, and
-- the sandbox proxy / workflows go through the SECURITY DEFINER functions below.
CREATE TABLE app.claude_credentials (
	owner       text        PRIMARY KEY,
	key_id      text        NOT NULL,
	ciphertext  bytea       NOT NULL,
	created_at  timestamptz NOT NULL DEFAULT now(),
	updated_at  timestamptz NOT NULL DEFAULT now()
);

-- app_api may be created or owned by another repo, in any order. Only create it when missing, never
-- alter its ownership. If the migration role cannot create it (no CREATE on the database), it must
-- already exist.
DO $$
BEGIN
	IF NOT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'app_api') THEN
		BEGIN
			CREATE SCHEMA app_api;
		EXCEPTION WHEN insufficient_privilege THEN
			RAISE EXCEPTION 'schema app_api does not exist and this role cannot create it; create it first';
		END;
	END IF;
END
$$;

-- Metadata only: no key_id, no ciphertext. Owner-run, so readers need no access to the table.
CREATE VIEW app_api.claude_credential_v AS
	SELECT c.owner, true AS present, c.updated_at
	FROM app.claude_credentials c;

-- Functions are owned by the migration role (which owns the table) and need CREATE on app_api.
-- Bodies use positional parameters and qualified names so they cannot be shadowed.
CREATE FUNCTION app_api.claude_credential_put(owner text, key_id text, ciphertext bytea, only_if_absent boolean)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, app
AS $$
	INSERT INTO app.claude_credentials AS t (owner, key_id, ciphertext)
	VALUES ($1, $2, $3)
	ON CONFLICT (owner) DO UPDATE
		SET key_id = EXCLUDED.key_id, ciphertext = EXCLUDED.ciphertext, updated_at = pg_catalog.now()
		WHERE NOT $4;
	SELECT EXISTS (SELECT 1 FROM app.claude_credentials t WHERE t.owner = $1);
$$;

CREATE FUNCTION app_api.claude_credential_delete(owner text)
RETURNS void
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, app
AS $$
	DELETE FROM app.claude_credentials t WHERE t.owner = $1;
$$;

CREATE FUNCTION app_api.claude_credential_get(owner text)
RETURNS TABLE (key_id text, ciphertext bytea)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, app
AS $$
	SELECT t.key_id, t.ciphertext FROM app.claude_credentials t WHERE t.owner = $1;
$$;

CREATE FUNCTION app_api.claude_credential_list_stale(active_key_id text)
RETURNS TABLE (owner text, key_id text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, app
AS $$
	SELECT t.owner, t.key_id FROM app.claude_credentials t WHERE t.key_id <> $1 ORDER BY t.owner;
$$;

-- Updates only while the row still has old_key_id, so a concurrent rekey or put wins. Returns whether
-- the row was updated.
CREATE FUNCTION app_api.claude_credential_rekey(owner text, old_key_id text, new_key_id text, ciphertext bytea)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, app
AS $$
	WITH u AS (
		UPDATE app.claude_credentials t
		SET key_id = $3, ciphertext = $4, updated_at = pg_catalog.now()
		WHERE t.owner = $1 AND t.key_id = $2
		RETURNING 1
	)
	SELECT EXISTS (SELECT 1 FROM u);
$$;

REVOKE ALL ON FUNCTION app_api.claude_credential_put(text, text, bytea, boolean) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.claude_credential_delete(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.claude_credential_get(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.claude_credential_list_stale(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.claude_credential_rekey(text, text, text, bytea) FROM PUBLIC;

-- Role grants and the enzarb_ui revokes live in scripts/grants.sql, which migrate re-runs every time
-- so roles provisioned after this migration still get them.
