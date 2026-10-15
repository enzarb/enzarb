-- Sandbox resource stats (ADR-0019 and its amendment, contracts.md section 6). Additive.
--
-- One row per poll of one sandbox, written by the poller (`enzarb_workflows`) with a plain
-- INSERT ... ON CONFLICT DO NOTHING; there is no insert function. Column names mirror the mux
-- `stats` frame. The cpu_*, io_* columns are monotonic counters (they restart with the sandbox);
-- memory and storage are gauges. A value the sandbox did not report is NULL, never 0. Rates are
-- derived at read time, so the reset and gap policy can change without rewriting data. No
-- foreign key to work_items: cleanup must not block or cascade, and `work_item_id` is
-- approximate and NULL while the sandbox is idle.
CREATE TABLE workflows.sandbox_stats (
    project             text        NOT NULL CHECK (project <> ''),
    sandbox             text        NOT NULL CHECK (sandbox <> ''),
    work_item_id        bigint,
    at                  timestamptz NOT NULL,
    cpu_usage_usec      bigint CHECK (cpu_usage_usec >= 0),
    cpu_user_usec       bigint CHECK (cpu_user_usec >= 0),
    cpu_system_usec     bigint CHECK (cpu_system_usec >= 0),
    memory_bytes        bigint CHECK (memory_bytes >= 0),
    io_read_bytes       bigint CHECK (io_read_bytes >= 0),
    io_write_bytes      bigint CHECK (io_write_bytes >= 0),
    storage_used_bytes  bigint CHECK (storage_used_bytes >= 0),
    storage_total_bytes bigint CHECK (storage_total_bytes >= 0),
    PRIMARY KEY (project, sandbox, at)
);
CREATE INDEX sandbox_stats_project_at_idx ON workflows.sandbox_stats (project, at);

-- Read-only view for enzarb_ui, which gets no access to the table.
CREATE VIEW workflows_api.sandbox_stats_v AS
    SELECT project, sandbox, work_item_id, at,
           cpu_usage_usec, cpu_user_usec, cpu_system_usec,
           memory_bytes, io_read_bytes, io_write_bytes,
           storage_used_bytes, storage_total_bytes
    FROM workflows.sandbox_stats
    ORDER BY at;

-- Bucketed stats of a project over the half-open window [p_from, p_to).
--
-- The bucket width is (p_to - p_from) / p_buckets; p_buckets is clamped to 1..300 and the
-- window to p_retention_days before p_to. A sample on a bucket edge belongs to the later
-- bucket. Buckets without samples return no row.
--
-- Rates come from counter deltas between consecutive samples of one sandbox, the last sample
-- before the window included so the first sample in it has a delta. The interval belongs to the
-- bucket of its end sample. CPU is in cores (delta usec over elapsed usec), I/O in bytes per
-- second. An interval is dropped (no rate) when its counter went down (a reset), when it is not
-- positive or longer than 3x the poll interval (a gap), or when the counter is NULL on either
-- side. A bucket's avg and max are over its usable intervals; gauges are avg and max over its
-- samples, and storage_total_bytes is the last reported value in the bucket.
--
-- p_interval_secs is the poller's ENZARB_STATS_INTERVAL (clamped 15..300) and p_retention_days
-- its ENZARB_STATS_RETENTION; the defaults are the poller's defaults, so callers may pass only
-- the first four arguments. Return columns are append-only.
CREATE FUNCTION workflows_api.sandbox_stats_window(
    p_project text, p_from timestamptz, p_to timestamptz, p_buckets integer,
    p_interval_secs integer DEFAULT 30, p_retention_days integer DEFAULT 30
) RETURNS TABLE (
    bucket_start           timestamptz,
    samples                integer,
    cpu_cores_avg          double precision,
    cpu_cores_max          double precision,
    mem_bytes_avg          double precision,
    mem_bytes_max          bigint,
    io_read_bps_avg        double precision,
    io_read_bps_max        double precision,
    io_write_bps_avg       double precision,
    io_write_bps_max       double precision,
    storage_used_bytes_avg double precision,
    storage_used_bytes_max bigint,
    storage_total_bytes    bigint
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, workflows, pg_temp
AS $$
#variable_conflict use_column
DECLARE
    v_n      integer;
    v_from   timestamptz;
    v_span   numeric;
    v_gap_us numeric;
BEGIN
    IF p_project IS NULL OR p_from IS NULL OR p_to IS NULL OR p_buckets IS NULL THEN
        RAISE EXCEPTION 'arguments must not be null' USING ERRCODE = '22023';
    END IF;
    IF p_to <= p_from THEN
        RAISE EXCEPTION 'empty window' USING ERRCODE = '22023';
    END IF;
    v_n := least(greatest(p_buckets, 1), 300);
    v_from := greatest(p_from,
                       p_to - make_interval(days => greatest(COALESCE(p_retention_days, 30), 1)));
    v_span := extract(epoch FROM (p_to - v_from))::numeric;
    v_gap_us := 3 * least(greatest(COALESCE(p_interval_secs, 30), 15), 300) * 1000000::numeric;

    RETURN QUERY
    WITH s AS (
        SELECT x.sandbox AS sb, x.at AS at, x.memory_bytes AS mem,
               x.storage_used_bytes AS su, x.storage_total_bytes AS st,
               x.cpu_usage_usec AS cpu, x.io_read_bytes AS dr, x.io_write_bytes AS dw,
               lag(x.at) OVER ws AS p_at, lag(x.cpu_usage_usec) OVER ws AS p_cpu,
               lag(x.io_read_bytes) OVER ws AS p_dr, lag(x.io_write_bytes) OVER ws AS p_dw
        FROM workflows.sandbox_stats x
        WHERE x.project = p_project
          AND x.at >= v_from - make_interval(secs => (v_gap_us / 1000000)::double precision)
          AND x.at < p_to
        WINDOW ws AS (PARTITION BY x.sandbox ORDER BY x.at)
    ),
    d AS (
        SELECT sb, at, mem, su, st, cpu, dr, dw, p_cpu, p_dr, p_dw,
               floor(extract(epoch FROM (at - v_from))::numeric * v_n / v_span)::integer AS idx,
               extract(epoch FROM (at - p_at))::numeric * 1000000 AS dt_us
        FROM s
        WHERE at >= v_from
    ),
    r AS (
        SELECT idx, at, mem, su, st,
               CASE WHEN dt_us > 0 AND dt_us <= v_gap_us AND cpu >= p_cpu
                    THEN (cpu - p_cpu)::double precision / dt_us::double precision END AS r_cpu,
               CASE WHEN dt_us > 0 AND dt_us <= v_gap_us AND dr >= p_dr
                    THEN (dr - p_dr)::double precision * 1000000 / dt_us::double precision END AS r_dr,
               CASE WHEN dt_us > 0 AND dt_us <= v_gap_us AND dw >= p_dw
                    THEN (dw - p_dw)::double precision * 1000000 / dt_us::double precision END AS r_dw
        FROM d
    )
    SELECT v_from + make_interval(secs => (r.idx * v_span / v_n)::double precision),
           count(*)::integer,
           avg(r.r_cpu), max(r.r_cpu),
           avg(r.mem)::double precision, max(r.mem),
           avg(r.r_dr), max(r.r_dr),
           avg(r.r_dw), max(r.r_dw),
           avg(r.su)::double precision, max(r.su),
           (array_agg(r.st ORDER BY r.at DESC) FILTER (WHERE r.st IS NOT NULL))[1]
    FROM r
    GROUP BY r.idx
    ORDER BY r.idx;
END;
$$;

REVOKE ALL ON FUNCTION workflows_api.sandbox_stats_window(text, timestamptz, timestamptz, integer, integer, integer) FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT SELECT, INSERT, DELETE ON workflows.sandbox_stats TO enzarb_workflows;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
        GRANT USAGE ON SCHEMA workflows_api TO enzarb_ui;
        GRANT SELECT ON workflows_api.sandbox_stats_v TO enzarb_ui;
        GRANT EXECUTE ON FUNCTION workflows_api.sandbox_stats_window(text, timestamptz, timestamptz, integer, integer, integer) TO enzarb_ui;
    END IF;
END
$$;
