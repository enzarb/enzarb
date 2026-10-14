-- Work item dependencies (docs/adr/0017-work-item-dependencies.md, docs/contracts.md section 6).
-- Additive only: new table, functions and views, plus one trailing column on work_item_v.
--
-- `work_item_dependencies` holds what each source declared; the engine never mixes sources:
--   github   GitHub's native "blocked by" relationships (synced from the connector)
--   text     `Depends on #N` / `Blocked by #N` in the item body
--   comment  the same phrases in comments (additive; a comment cannot retract another's)
-- Keys are lowercase work item keys (`github:owner/repo#12`). A dependency may name an item
-- that has no work item yet; it stays `pending` until one exists. The engine only writes
-- dependencies inside one Project; that check lives in the reducer, which knows the Projects.

CREATE TABLE workflows.work_item_dependencies (
    work_item_key  text        NOT NULL,
    depends_on_key text        NOT NULL,
    source         text        NOT NULL CHECK (source IN ('github', 'text', 'comment')),
    created_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (work_item_key, depends_on_key, source),
    CHECK (work_item_key <> depends_on_key)
);
CREATE INDEX work_item_dependencies_dependent_idx
    ON workflows.work_item_dependencies (depends_on_key);

-- Set `p_key`'s dependencies from `p_source` to `p_deps` (self references are dropped). With
-- `p_replace` false only missing rows are added. Returns true when anything changed.
CREATE FUNCTION workflows.set_dependencies(
    p_key text, p_source text, p_deps text[], p_replace boolean DEFAULT true
) RETURNS boolean
LANGUAGE plpgsql AS $$
DECLARE
    changed boolean := false;
    n bigint;
BEGIN
    IF p_replace THEN
        DELETE FROM workflows.work_item_dependencies
        WHERE work_item_key = p_key AND source = p_source
          AND NOT (depends_on_key = ANY (p_deps));
        GET DIAGNOSTICS n = ROW_COUNT;
        changed := n > 0;
    END IF;
    INSERT INTO workflows.work_item_dependencies (work_item_key, depends_on_key, source)
    SELECT DISTINCT p_key, d, p_source FROM unnest(p_deps) AS d WHERE d <> p_key
    ON CONFLICT DO NOTHING;
    GET DIAGNOSTICS n = ROW_COUNT;
    RETURN changed OR n > 0;
END
$$;

-- What each item waits on: its own rows, plus (for a change request whose body says
-- `Closes #N`, `Fixes #N` or `Resolves #N`) the rows of that issue in the same repository.
-- Computed here rather than copied, so the inherited set never goes stale.
CREATE VIEW workflows.effective_dependencies AS
    SELECT work_item_key, depends_on_key, source, NULL::text AS via
    FROM workflows.work_item_dependencies
    UNION
    SELECT w.key, d.depends_on_key, d.source, d.work_item_key
    FROM workflows.work_items w
    CROSS JOIN LATERAL regexp_matches(
        coalesce(w.state_data->>'body', ''),
        '(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#([0-9]+)', 'gi') AS m(g)
    JOIN workflows.work_item_dependencies d
      ON d.work_item_key = regexp_replace(w.key, '#[0-9]+$', '') || '#' || m.g[1]
    WHERE w.workflow = 'change_request' AND d.depends_on_key <> w.key;

-- Each effective dependency with the state of the item it names:
--   done     the item is Done or Merged
--   closed   it was cancelled, closed unmerged or a duplicate: it will not complete
--   open     it exists and is still in progress
--   pending  no work item for it yet
CREATE VIEW workflows.dependency_status AS
    SELECT e.work_item_key, e.depends_on_key, e.source, e.via,
           CASE WHEN dw.id IS NULL THEN 'pending'
                WHEN dw.state IN ('Done', 'Merged') THEN 'done'
                WHEN dw.state IN ('Cancelled', 'Closed', 'Duplicate') THEN 'closed'
                ELSE 'open' END AS status,
           dw.state AS state,
           dw.state_data->>'title' AS title,
           dw.state_data->>'url' AS url
    FROM workflows.effective_dependencies e
    LEFT JOIN workflows.work_items dw ON dw.key = e.depends_on_key;

-- A shortest dependency cycle through `p_key` as the list of keys from `p_key` back to
-- `p_key`, or NULL when it is on none.
CREATE FUNCTION workflows.dependency_cycle(p_key text) RETURNS text[]
LANGUAGE sql STABLE AS $$
    WITH RECURSIVE walk(node, path) AS (
        SELECT e.depends_on_key, ARRAY[e.work_item_key, e.depends_on_key]
        FROM workflows.effective_dependencies e
        WHERE e.work_item_key = p_key
      UNION ALL
        SELECT e.depends_on_key, w.path || e.depends_on_key
        FROM walk w
        JOIN workflows.effective_dependencies e ON e.work_item_key = w.node
        WHERE w.node <> p_key
          AND (e.depends_on_key = p_key OR NOT (e.depends_on_key = ANY (w.path)))
    )
    SELECT path FROM walk WHERE node = p_key ORDER BY cardinality(path) LIMIT 1
$$;

-- Both directions for the UI: filter on work_item_key for what an item waits on, on
-- depends_on_key for its dependents.
CREATE VIEW workflows_api.work_item_dependency_v AS
    SELECT work_item_key, depends_on_key, source, via, status, state, title, url
    FROM workflows.dependency_status;

-- Append `blocked_by_dependency`: the item's stored open dependencies (written by the engine
-- into state_data) are not empty.
CREATE OR REPLACE VIEW workflows_api.work_item_v AS
    SELECT w.id, w.key, w.project_id AS project, w.workflow, w.state, w.state_data,
           w.created_at, w.updated_at,
           wj.waiting_for, wj.resumes_at,
           ut.usage_totals,
           w.state_data #>> '{wait,waiting_on}' AS waiting_on,
           w.state_data #>> '{wait,reason}' AS wait_reason,
           w.state_data #>> '{wait,text}' AS wait_text,
           (CASE WHEN jsonb_typeof(w.state_data->'open_dependencies') = 'array'
                 THEN jsonb_array_length(w.state_data->'open_dependencies') > 0
                 ELSE false END) AS blocked_by_dependency
    FROM workflows.work_items w
    LEFT JOIN LATERAL (
        SELECT j.waiting_for, j.resumes_at
        FROM workflows.jobs j
        WHERE j.work_item_id = w.id AND j.status = 'queued' AND j.waiting_for IS NOT NULL
        ORDER BY j.resumes_at DESC NULLS LAST, j.id DESC
        LIMIT 1
    ) wj ON true
    LEFT JOIN LATERAL (
        SELECT NULLIF(jsonb_strip_nulls(jsonb_build_object(
                   'input_tokens',                sum((u.usage->>'input_tokens')::bigint),
                   'output_tokens',               sum((u.usage->>'output_tokens')::bigint),
                   'cache_read_input_tokens',     sum((u.usage->>'cache_read_input_tokens')::bigint),
                   'cache_creation_input_tokens', sum((u.usage->>'cache_creation_input_tokens')::bigint),
                   'cost_usd',                    sum((u.usage->>'cost_usd')::numeric),
                   'models', (
                       SELECT jsonb_agg(DISTINCT m.model)
                       FROM workflows.jobs m_j
                       CROSS JOIN LATERAL jsonb_array_elements_text(
                           CASE WHEN jsonb_typeof(m_j.usage->'models') = 'array'
                                THEN m_j.usage->'models' ELSE '[]'::jsonb END) AS m(model)
                       WHERE m_j.work_item_id = w.id
                   )
               )), '{}'::jsonb) AS usage_totals
        FROM workflows.jobs u
        WHERE u.work_item_id = w.id AND u.usage IS NOT NULL
    ) ut ON true;

-- The dependency view shows both ends, so a change wakes both work items (see the header of
-- 20261011000001_notify_work_item_changed.sql).
CREATE FUNCTION workflows.notify_dependency_changed() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    r workflows.work_item_dependencies;
BEGIN
    IF TG_OP = 'DELETE' THEN
        r := OLD;
    ELSE
        r := NEW;
    END IF;
    PERFORM pg_notify('work_item_changed', w.id::text)
    FROM workflows.work_items w
    WHERE w.key IN (r.work_item_key, r.depends_on_key);
    RETURN r;
END
$$;

CREATE TRIGGER work_item_dependencies_notify
    AFTER INSERT OR DELETE ON workflows.work_item_dependencies
    FOR EACH ROW EXECUTE FUNCTION workflows.notify_dependency_changed();

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
        GRANT USAGE ON SCHEMA workflows_api TO enzarb_ui;
        GRANT SELECT ON workflows_api.work_item_dependency_v TO enzarb_ui;
    ELSE
        RAISE NOTICE 'role enzarb_ui missing; grants skipped';
    END IF;
END
$$;
