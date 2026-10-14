-- Web Push (issue #79). owner is the GitHub login (no FK, as for claude_credentials).
-- Neither table is reachable by the UI role: every access goes through the SECURITY DEFINER
-- functions below, and each takes the owner explicitly. The UI passes the authenticated session login
-- (never client input); the DB cannot tell users apart because the UI role shares one pool.

-- One row per device. The endpoint is globally unique: a different owner cannot take it over.
CREATE TABLE app.push_subscriptions (
	id           bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
	owner        text        NOT NULL,
	endpoint     text        NOT NULL UNIQUE,
	p256dh       text        NOT NULL,
	auth         text        NOT NULL,
	categories   text[]      NOT NULL,
	user_agent   text,
	created_at   timestamptz NOT NULL DEFAULT now(),
	last_used_at timestamptz
);
CREATE INDEX push_subscriptions_owner_idx ON app.push_subscriptions (owner);

-- Sent-log: the primary key is the cross-pod de-duplication claim (INSERT ... ON CONFLICT DO NOTHING).
CREATE TABLE app.push_sent (
	owner        text        NOT NULL,
	work_item_id bigint      NOT NULL,
	category     text        NOT NULL,
	state_key    text        NOT NULL,
	sent_at      timestamptz NOT NULL DEFAULT now(),
	PRIMARY KEY (owner, work_item_id, category, state_key)
);
CREATE INDEX push_sent_owner_sent_at_idx ON app.push_sent (owner, sent_at);
CREATE INDEX push_sent_sent_at_idx ON app.push_sent (sent_at);

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

-- Registers or refreshes a device. Returns false (and changes nothing) when the endpoint already
-- belongs to another owner.
CREATE FUNCTION app_api.push_subscription_upsert(
	owner text, endpoint text, p256dh text, auth text, categories text[], user_agent text)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	INSERT INTO app.push_subscriptions AS t (owner, endpoint, p256dh, auth, categories, user_agent)
	VALUES ($1, $2, $3, $4, $5, $6)
	ON CONFLICT (endpoint) DO UPDATE
		SET p256dh = EXCLUDED.p256dh, auth = EXCLUDED.auth,
			categories = EXCLUDED.categories, user_agent = EXCLUDED.user_agent
		WHERE t.owner = EXCLUDED.owner;
	SELECT EXISTS (SELECT 1 FROM app.push_subscriptions t WHERE t.endpoint = $2 AND t.owner = $1);
$$;

-- The owner's devices for the Settings page: no keys.
CREATE FUNCTION app_api.push_subscription_list(owner text)
RETURNS TABLE (endpoint text, categories text[], user_agent text, created_at timestamptz)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	SELECT t.endpoint, t.categories, t.user_agent, t.created_at
	FROM app.push_subscriptions t WHERE t.owner = $1 ORDER BY t.created_at, t.id;
$$;

-- Sender side: the owner's devices subscribed to a category (all of them when category is null).
CREATE FUNCTION app_api.push_subscription_targets(owner text, category text)
RETURNS TABLE (endpoint text, p256dh text, auth text)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	SELECT t.endpoint, t.p256dh, t.auth FROM app.push_subscriptions t
	WHERE t.owner = $1 AND ($2 IS NULL OR $2 = ANY (t.categories)) ORDER BY t.id;
$$;

CREATE FUNCTION app_api.push_subscription_set_categories(owner text, endpoint text, categories text[])
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	WITH u AS (
		UPDATE app.push_subscriptions t SET categories = $3
		WHERE t.owner = $1 AND t.endpoint = $2 RETURNING 1
	)
	SELECT EXISTS (SELECT 1 FROM u);
$$;

-- Own device removal (Settings "disable") and pruning after a 404/410 from the push service.
CREATE FUNCTION app_api.push_subscription_delete(owner text, endpoint text)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	WITH d AS (
		DELETE FROM app.push_subscriptions t WHERE t.owner = $1 AND t.endpoint = $2 RETURNING 1
	)
	SELECT EXISTS (SELECT 1 FROM d);
$$;

CREATE FUNCTION app_api.push_subscription_touch(owner text, endpoint text)
RETURNS void
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	UPDATE app.push_subscriptions t SET last_used_at = pg_catalog.now()
	WHERE t.owner = $1 AND t.endpoint = $2;
$$;

-- De-duplication claim: true only for the first caller of a (owner, item, category, state_key).
-- Also returns how many claims the owner made in the last window_secs (this one included), which the
-- sender uses to collapse bursts. Both are multi-pod safe because they read the same table.
CREATE FUNCTION app_api.push_claim(owner text, work_item_id bigint, category text, state_key text, window_secs integer)
RETURNS TABLE (claimed boolean, recent integer)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	WITH c AS (
		INSERT INTO app.push_sent (owner, work_item_id, category, state_key)
		VALUES ($1, $2, $3, $4)
		ON CONFLICT DO NOTHING
		RETURNING 1
	)
	SELECT EXISTS (SELECT 1 FROM c),
		-- The INSERT is not visible to other parts of the same statement, so add this claim by hand.
		(SELECT pg_catalog.count(*) FROM app.push_sent s
			WHERE s.owner = $1 AND s.sent_at > pg_catalog.now() - pg_catalog.make_interval(secs => $5))::integer
			+ (SELECT pg_catalog.count(*) FROM c)::integer;
$$;

-- Releases a claim so a later update can notify again (nothing was delivered). Returns whether a row went.
CREATE FUNCTION app_api.push_claim_release(owner text, work_item_id bigint, category text, state_key text)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	WITH d AS (
		DELETE FROM app.push_sent s
		WHERE s.owner = $1 AND s.work_item_id = $2 AND s.category = $3 AND s.state_key = $4
		RETURNING 1
	)
	SELECT EXISTS (SELECT 1 FROM d);
$$;

-- Deletes sent-log rows older than the given number of days; returns how many.
CREATE FUNCTION app_api.push_sent_prune(older_than_days integer)
RETURNS integer
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	WITH d AS (
		DELETE FROM app.push_sent s
		WHERE s.sent_at < pg_catalog.now() - pg_catalog.make_interval(days => $1) RETURNING 1
	)
	SELECT pg_catalog.count(*)::integer FROM d;
$$;

REVOKE ALL ON FUNCTION app_api.push_subscription_upsert(text, text, text, text, text[], text) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_subscription_list(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_subscription_targets(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_subscription_set_categories(text, text, text[]) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_subscription_delete(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_subscription_touch(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_claim(text, bigint, text, text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_claim_release(text, bigint, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_sent_prune(integer) FROM PUBLIC;

-- Grants to enzarb_ui live in scripts/grants.sql (re-run on every migrate).
