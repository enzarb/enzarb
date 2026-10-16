-- Web Push: the open "needs you" entry per (owner, work item), so a category that stays does not
-- notify again and leaving it sends one clear push (docs/notifications.md). Reachable only through the
-- SECURITY DEFINER functions below, like the 0006 tables.
CREATE TABLE app.push_item (
	owner        text        NOT NULL,
	work_item_id bigint      NOT NULL,
	category     text        NOT NULL,
	state_key    text        NOT NULL,
	-- True when a notification with tag item-<id> was shown for this entry (not only a burst summary).
	shown        boolean     NOT NULL,
	updated_at   timestamptz NOT NULL DEFAULT now(),
	-- Set once the entry was left (and, if shown, the clear push was claimed).
	cleared_at   timestamptz,
	PRIMARY KEY (owner, work_item_id)
);
CREATE INDEX push_item_updated_at_idx ON app.push_item (updated_at);

-- The owner's open (not cleared) entry for a work item, if any.
CREATE FUNCTION app_api.push_item_get(owner text, work_item_id bigint)
RETURNS TABLE (category text, state_key text, shown boolean)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	SELECT p.category, p.state_key, p.shown FROM app.push_item p
	WHERE p.owner = $1 AND p.work_item_id = $2 AND p.cleared_at IS NULL;
$$;

-- Records a new entry after a delivery (replaces any earlier one).
CREATE FUNCTION app_api.push_item_set(owner text, work_item_id bigint, category text, state_key text, shown boolean)
RETURNS void
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	INSERT INTO app.push_item (owner, work_item_id, category, state_key, shown)
	VALUES ($1, $2, $3, $4, $5)
	ON CONFLICT (owner, work_item_id) DO UPDATE
		SET category = EXCLUDED.category, state_key = EXCLUDED.state_key, shown = EXCLUDED.shown,
			updated_at = pg_catalog.now(), cleared_at = NULL;
$$;

-- Closes the open entry. True only for the one caller that closed it, and only when its notification
-- was shown, so the clear push goes out once across pods.
CREATE FUNCTION app_api.push_item_clear(owner text, work_item_id bigint)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	WITH u AS (
		UPDATE app.push_item p SET cleared_at = pg_catalog.now(), updated_at = pg_catalog.now()
		WHERE p.owner = $1 AND p.work_item_id = $2 AND p.cleared_at IS NULL
		RETURNING p.shown
	)
	SELECT COALESCE((SELECT u.shown FROM u), false);
$$;

-- Deletes entries untouched for the given number of days; returns how many.
CREATE FUNCTION app_api.push_item_prune(older_than_days integer)
RETURNS integer
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
	WITH d AS (
		DELETE FROM app.push_item p
		WHERE p.updated_at < pg_catalog.now() - pg_catalog.make_interval(days => $1) RETURNING 1
	)
	SELECT pg_catalog.count(*)::integer FROM d;
$$;

REVOKE ALL ON FUNCTION app_api.push_item_get(text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_item_set(text, bigint, text, text, boolean) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_item_clear(text, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION app_api.push_item_prune(integer) FROM PUBLIC;

-- Grants to enzarb_ui live in scripts/grants.sql (re-run on every migrate).
