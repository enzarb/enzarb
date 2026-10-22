-- Re-assertable grants, run by scripts/migrate.ts after the migrations on every run (not tracked in
-- app._migrations). Runtime roles are provisioned by other components (the helm pgop Roles) and may appear
-- after a migration ran, so every statement here is idempotent and guarded by pg_roles; the next
-- migrate run picks up roles that were missing before. Ordering requirement: create the Roles
-- before running the UI migrate Job, otherwise the grants below are skipped (with a NOTICE) until
-- the next run.
DO $$
DECLARE
	r text;
BEGIN
	FOREACH r IN ARRAY ARRAY['enzarb_sandbox_proxy', 'enzarb_ui'] LOOP
		IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = r) THEN
			RAISE NOTICE 'role % does not exist; its grants are skipped until the next migrate run', r;
		END IF;
	END LOOP;

	IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_sandbox_proxy') THEN
		GRANT USAGE ON SCHEMA app_api TO enzarb_sandbox_proxy;
		GRANT EXECUTE ON FUNCTION app_api.claude_credential_put(text, text, bytea, boolean) TO enzarb_sandbox_proxy;
		GRANT EXECUTE ON FUNCTION app_api.claude_credential_delete(text) TO enzarb_sandbox_proxy;
		GRANT EXECUTE ON FUNCTION app_api.claude_credential_get(text) TO enzarb_sandbox_proxy;
		GRANT EXECUTE ON FUNCTION app_api.claude_credential_list_stale(text) TO enzarb_sandbox_proxy;
		GRANT EXECUTE ON FUNCTION app_api.claude_credential_rekey(text, text, text, bytea) TO enzarb_sandbox_proxy;
	END IF;

	IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
		-- Default privileges from 0001 give the runtime role DML on new tables; take it away again.
		REVOKE ALL ON app.claude_credentials FROM enzarb_ui;
		-- Also covers app_api default privileges added elsewhere.
		REVOKE ALL ON FUNCTION app_api.claude_credential_put(text, text, bytea, boolean) FROM enzarb_ui;
		REVOKE ALL ON FUNCTION app_api.claude_credential_delete(text) FROM enzarb_ui;
		REVOKE ALL ON FUNCTION app_api.claude_credential_get(text) FROM enzarb_ui;
		REVOKE ALL ON FUNCTION app_api.claude_credential_list_stale(text) FROM enzarb_ui;
		REVOKE ALL ON FUNCTION app_api.claude_credential_rekey(text, text, text, bytea) FROM enzarb_ui;
		GRANT USAGE ON SCHEMA app_api TO enzarb_ui;
		GRANT SELECT ON app_api.claude_credential_v TO enzarb_ui;
		-- The one function the UI may run: deletes the given owner's row, returns a boolean.
		GRANT EXECUTE ON FUNCTION app_api.claude_credential_delete_own(text) TO enzarb_ui;
		-- Web Push (0006): the tables are reachable only through these functions.
		REVOKE ALL ON app.push_subscriptions FROM enzarb_ui;
		REVOKE ALL ON app.push_sent FROM enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_subscription_upsert(text, text, text, text, text[], text) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_subscription_list(text) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_subscription_targets(text, text) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_subscription_set_categories(text, text, text[]) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_subscription_delete(text, text) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_subscription_touch(text, text) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_claim(text, bigint, text, text, integer) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_claim_release(text, bigint, text, text) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_sent_prune(integer) TO enzarb_ui;
		-- Web Push entries (0007).
		REVOKE ALL ON app.push_item FROM enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_item_get(text, bigint) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_item_set(text, bigint, text, text, boolean) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_item_clear(text, bigint) TO enzarb_ui;
		GRANT EXECUTE ON FUNCTION app_api.push_item_prune(integer) TO enzarb_ui;
	END IF;
END
$$;
