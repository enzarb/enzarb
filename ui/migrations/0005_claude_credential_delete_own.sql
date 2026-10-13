-- Lets the UI remove the signed-in user's own Claude token without a running sandbox. The UI role
-- shares one pool, so the DB cannot tell users apart: the caller (claude.remote.ts) passes the
-- authenticated login, never client input. Returns only whether a row was removed, never key_id or
-- ciphertext. Granted to enzarb_ui in scripts/grants.sql.
CREATE FUNCTION app_api.claude_credential_delete_own(owner text)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, app
AS $$
	WITH d AS (
		DELETE FROM app.claude_credentials t WHERE t.owner = $1 RETURNING 1
	)
	SELECT EXISTS (SELECT 1 FROM d);
$$;

REVOKE ALL ON FUNCTION app_api.claude_credential_delete_own(text) FROM PUBLIC;
