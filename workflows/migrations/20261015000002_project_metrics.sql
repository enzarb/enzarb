-- Project metrics (contracts.md section 6): windowed usage/cost and step timing.
-- Additive: two new read-only functions. SECURITY DEFINER like the other functions, so the
-- reader needs no access to schema `workflows`.

-- Usage per project over work items created in [p_from, p_to). One `total` row plus one `task`
-- row per work item. A field a work item did not report is NULL ("not reported"), never 0:
-- sums, averages and medians only count the items that reported it (`*_reporters`), and are
-- NULL when none did.
CREATE FUNCTION workflows_api.project_usage(
    p_project text, p_from timestamptz, p_to timestamptz
) RETURNS TABLE (
    kind                        text,
    work_item_key               text,
    state                       text,
    items                       bigint,
    input_tokens                numeric,
    output_tokens               numeric,
    cache_read_input_tokens     numeric,
    cache_creation_input_tokens numeric,
    cost_usd                    numeric,
    input_tokens_reporters      bigint,
    output_tokens_reporters     bigint,
    cost_usd_reporters          bigint,
    input_tokens_avg            numeric,
    output_tokens_avg           numeric,
    cost_usd_avg                numeric,
    input_tokens_median         double precision,
    output_tokens_median        double precision,
    cost_usd_median             double precision
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, workflows, pg_temp
AS $$
    WITH w AS (
        SELECT v.id, v.key, v.state,
               (v.usage_totals ->> 'input_tokens')::numeric                AS i,
               (v.usage_totals ->> 'output_tokens')::numeric               AS o,
               (v.usage_totals ->> 'cache_read_input_tokens')::numeric     AS cr,
               (v.usage_totals ->> 'cache_creation_input_tokens')::numeric AS cc,
               (v.usage_totals ->> 'cost_usd')::numeric                    AS c
        FROM workflows_api.work_item_v v
        WHERE v.project = p_project AND v.created_at >= p_from AND v.created_at < p_to
    )
    SELECT 'total'::text, NULL::text, NULL::text, count(*),
           sum(i), sum(o), sum(cr), sum(cc), sum(c),
           count(i), count(o), count(c),
           avg(i), avg(o), avg(c),
           percentile_cont(0.5) WITHIN GROUP (ORDER BY i::double precision),
           percentile_cont(0.5) WITHIN GROUP (ORDER BY o::double precision),
           percentile_cont(0.5) WITHIN GROUP (ORDER BY c::double precision)
    FROM w
    UNION ALL
    SELECT * FROM (
        SELECT 'task'::text, key, state, 1::bigint,
               i, o, cr, cc, c,
               NULL::bigint, NULL::bigint, NULL::bigint,
               NULL::numeric, NULL::numeric, NULL::numeric,
               NULL::double precision, NULL::double precision, NULL::double precision
        FROM w ORDER BY id
    ) t;
$$;

-- Step timing per project. `state` rows: the time a work item spent in each state, from the
-- transition that entered it (`transitions.at` in [p_from, p_to)) to the next transition of the
-- same item. `job` rows: finished jobs by `jobs.kind`, `created_at` to `updated_at`, for jobs
-- created in the window. Durations are seconds.
-- The last (open) state of an item has no end: it is excluded from the statistics and counted
-- in `open_excluded`, unless p_include_open (then it runs to now()). An open state that is
-- currently waiting (`state_data.wait`) is never a sample; it is counted in `wait_count` with
-- its elapsed time in `wait_s`. Past waits are not recorded on transitions and cannot be
-- separated from active time.
CREATE FUNCTION workflows_api.project_step_timing(
    p_project text, p_from timestamptz, p_to timestamptz, p_include_open boolean DEFAULT false
) RETURNS TABLE (
    kind          text,
    name          text,
    n             bigint,
    open_excluded bigint,
    wait_count    bigint,
    wait_s        double precision,
    min_s         double precision,
    max_s         double precision,
    avg_s         double precision,
    median_s      double precision,
    p95_s         double precision
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, workflows, pg_temp
AS $$
    WITH t AS (
        SELECT x.work_item_id, x.to_state, x.at,
               lead(x.at) OVER (PARTITION BY x.work_item_id ORDER BY x.id) AS next_at
        FROM workflows.transitions x
        JOIN workflows.work_items w ON w.id = x.work_item_id
        WHERE w.project_id = p_project
    ), s AS (
        SELECT t.to_state, t.next_at IS NULL AS is_open,
               (t.next_at IS NULL AND w.state_data #>> '{wait,waiting_on}' IS NOT NULL) AS waiting,
               extract(epoch FROM coalesce(t.next_at, now()) - t.at)::double precision AS secs
        FROM t JOIN workflows.work_items w ON w.id = t.work_item_id
        WHERE t.at >= p_from AND t.at < p_to
    ), a AS (
        SELECT to_state, is_open, waiting, secs,
               (NOT waiting AND (NOT is_open OR p_include_open)) AS counted
        FROM s
    )
    SELECT 'state'::text, to_state, count(*) FILTER (WHERE counted),
           count(*) FILTER (WHERE is_open AND NOT waiting AND NOT p_include_open),
           count(*) FILTER (WHERE waiting), sum(secs) FILTER (WHERE waiting),
           min(secs) FILTER (WHERE counted), max(secs) FILTER (WHERE counted),
           avg(secs) FILTER (WHERE counted),
           percentile_cont(0.5) WITHIN GROUP (ORDER BY secs) FILTER (WHERE counted),
           percentile_cont(0.95) WITHIN GROUP (ORDER BY secs) FILTER (WHERE counted)
    FROM a GROUP BY to_state
    UNION ALL
    SELECT 'job'::text, j.kind, count(*), 0::bigint, 0::bigint, NULL::double precision,
           min(d), max(d), avg(d),
           percentile_cont(0.5) WITHIN GROUP (ORDER BY d),
           percentile_cont(0.95) WITHIN GROUP (ORDER BY d)
    FROM (
        SELECT j.kind, extract(epoch FROM j.updated_at - j.created_at)::double precision AS d
        FROM workflows.jobs j
        JOIN workflows.work_items w ON w.id = j.work_item_id
        WHERE w.project_id = p_project AND j.created_at >= p_from AND j.created_at < p_to
          AND j.status IN ('succeeded', 'failed', 'cancelled')
    ) j
    GROUP BY j.kind
    ORDER BY 1, 2;
$$;

REVOKE ALL ON FUNCTION workflows_api.project_usage(text, timestamptz, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION workflows_api.project_step_timing(text, timestamptz, timestamptz, boolean) FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
        GRANT EXECUTE ON FUNCTION workflows_api.project_usage(text, timestamptz, timestamptz) TO enzarb_ui;
        GRANT EXECUTE ON FUNCTION workflows_api.project_step_timing(text, timestamptz, timestamptz, boolean) TO enzarb_ui;
    ELSE
        RAISE NOTICE 'role enzarb_ui missing; grants skipped';
    END IF;
END
$$;
