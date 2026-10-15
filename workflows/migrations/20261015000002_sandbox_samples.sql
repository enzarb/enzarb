-- Sandbox resource samples (epic: project metrics). Additive.
--
-- One row per poll of one sandbox. CPU time and disk bytes are monotonic counters (they reset
-- when the sandbox restarts); memory and storage are gauges. Rates are derived at read time
-- (`workflows_api.sandbox_series`), so the reset policy and bucket size can change without
-- rewriting data. No foreign key to work_items: work item cleanup must not block or cascade,
-- and `work_item_id` is NULL while the sandbox is idle. Not wired to `work_item_changed`.
CREATE TABLE workflows.sandbox_samples (
    id               bigserial PRIMARY KEY,
    project_id       text        NOT NULL CHECK (project_id <> ''),
    work_item_id     bigint,
    sandbox          text        NOT NULL CHECK (sandbox <> ''),
    sampled_at       timestamptz NOT NULL,
    cpu_usage_ns     bigint      NOT NULL CHECK (cpu_usage_ns >= 0),
    memory_bytes     bigint      NOT NULL CHECK (memory_bytes >= 0),
    storage_bytes    bigint      NOT NULL CHECK (storage_bytes >= 0),
    disk_read_bytes  bigint      NOT NULL CHECK (disk_read_bytes >= 0),
    disk_write_bytes bigint      NOT NULL CHECK (disk_write_bytes >= 0),
    -- A re-polled sample is a no-op; also the per-sandbox window and lag index.
    UNIQUE (project_id, sandbox, sampled_at)
);
CREATE INDEX sandbox_samples_project_time_idx ON workflows.sandbox_samples (project_id, sampled_at);
CREATE INDEX sandbox_samples_time_idx ON workflows.sandbox_samples (sampled_at);

-- Sole writer: the poller. Returns false when the (project, sandbox, sampled_at) sample exists.
CREATE FUNCTION workflows_api.record_sandbox_sample(
    p_project text, p_work_item_id bigint, p_sandbox text, p_sampled_at timestamptz,
    p_cpu_ns bigint, p_mem bigint, p_storage bigint, p_disk_read bigint, p_disk_write bigint
) RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, workflows, pg_temp
AS $$
DECLARE
    v_id bigint;
BEGIN
    IF p_project IS NULL OR p_project = '' THEN
        RAISE EXCEPTION 'project must not be empty' USING ERRCODE = '22023';
    END IF;
    IF p_sandbox IS NULL OR p_sandbox = '' THEN
        RAISE EXCEPTION 'sandbox must not be empty' USING ERRCODE = '22023';
    END IF;
    IF p_sampled_at IS NULL THEN
        RAISE EXCEPTION 'sampled_at must not be null' USING ERRCODE = '22023';
    END IF;
    IF p_sampled_at > now() + interval '5 minutes' THEN
        RAISE EXCEPTION 'sampled_at is in the future' USING ERRCODE = '22023';
    END IF;
    IF p_cpu_ns IS NULL OR p_mem IS NULL OR p_storage IS NULL
       OR p_disk_read IS NULL OR p_disk_write IS NULL THEN
        RAISE EXCEPTION 'sample values must not be null' USING ERRCODE = '22023';
    END IF;
    IF p_cpu_ns < 0 OR p_mem < 0 OR p_storage < 0 OR p_disk_read < 0 OR p_disk_write < 0 THEN
        RAISE EXCEPTION 'sample values must not be negative' USING ERRCODE = '22023';
    END IF;

    INSERT INTO workflows.sandbox_samples
        (project_id, work_item_id, sandbox, sampled_at, cpu_usage_ns, memory_bytes,
         storage_bytes, disk_read_bytes, disk_write_bytes)
    VALUES (p_project, p_work_item_id, p_sandbox, p_sampled_at, p_cpu_ns, p_mem,
            p_storage, p_disk_read, p_disk_write)
    ON CONFLICT (project_id, sandbox, sampled_at) DO NOTHING
    RETURNING id INTO v_id;
    RETURN v_id IS NOT NULL;
END;
$$;

-- Bucketed series of a project over the half-open window [p_from, p_to).
--
-- Buckets start at p_from and are p_bucket wide (the last may be partial); every bucket is
-- returned, empty ones with NULL values. Rates come from counter deltas between consecutive
-- samples of one sandbox, the last sample before p_from included so the first sample in the
-- window has a delta. A counter that went down is a reset: the delta is the new value. An
-- interval is attributed to the bucket of its end sample and ignored when it is not positive
-- or longer than 5 minutes (a long gap can hide several resets). A sandbox's rate in a bucket
-- is its summed delta over its summed interval; the project's rate is the sum over sandboxes.
-- Gauges are per-sandbox avg/max in the bucket, summed over sandboxes.
CREATE FUNCTION workflows_api.sandbox_series(
    p_project text, p_from timestamptz, p_to timestamptz, p_bucket interval
) RETURNS TABLE (
    bucket            timestamptz,
    cpu_cores         double precision,
    memory_bytes_avg  double precision,
    memory_bytes_max  bigint,
    storage_bytes_max bigint,
    disk_read_bps     double precision,
    disk_write_bps    double precision,
    samples           integer
)
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, workflows, pg_temp
AS $$
#variable_conflict use_column
DECLARE
    c_max_buckets constant integer := 1000;
    c_max_gap     constant interval := interval '5 minutes';
BEGIN
    IF p_project IS NULL OR p_from IS NULL OR p_to IS NULL OR p_bucket IS NULL THEN
        RAISE EXCEPTION 'arguments must not be null' USING ERRCODE = '22023';
    END IF;
    IF p_to <= p_from THEN
        RAISE EXCEPTION 'empty window' USING ERRCODE = '22023';
    END IF;
    IF p_bucket < interval '1 second' THEN
        RAISE EXCEPTION 'bucket must be at least 1 second' USING ERRCODE = '22023';
    END IF;
    IF extract(epoch FROM (p_to - p_from)) / extract(epoch FROM p_bucket) > c_max_buckets THEN
        RAISE EXCEPTION 'too many buckets (max %)', c_max_buckets USING ERRCODE = '22023';
    END IF;

    RETURN QUERY
    WITH s AS (
        SELECT x.sandbox AS sb, x.sampled_at AS at, x.memory_bytes AS mem, x.storage_bytes AS sto,
               x.cpu_usage_ns AS cpu, x.disk_read_bytes AS dr, x.disk_write_bytes AS dw,
               lag(x.sampled_at) OVER ws AS p_at, lag(x.cpu_usage_ns) OVER ws AS p_cpu,
               lag(x.disk_read_bytes) OVER ws AS p_dr, lag(x.disk_write_bytes) OVER ws AS p_dw
        FROM workflows.sandbox_samples x
        WHERE x.project_id = p_project
          AND x.sampled_at >= p_from - c_max_gap AND x.sampled_at < p_to
        WINDOW ws AS (PARTITION BY x.sandbox ORDER BY x.sampled_at)
    ),
    d AS (
        SELECT sb, date_bin(p_bucket, at, p_from) AS b, mem, sto,
               extract(epoch FROM (at - p_at)) AS dt,
               CASE WHEN cpu >= p_cpu THEN cpu - p_cpu ELSE cpu END AS d_cpu,
               CASE WHEN dr >= p_dr THEN dr - p_dr ELSE dr END AS d_dr,
               CASE WHEN dw >= p_dw THEN dw - p_dw ELSE dw END AS d_dw
        FROM s
        WHERE at >= p_from
    ),
    per_sb AS (
        SELECT sb, b,
               avg(mem) AS mem_avg, max(mem) AS mem_max, max(sto) AS sto_max, count(*) AS n,
               sum(dt) FILTER (WHERE dt > 0 AND dt <= extract(epoch FROM c_max_gap)) AS secs,
               sum(d_cpu) FILTER (WHERE dt > 0 AND dt <= extract(epoch FROM c_max_gap)) AS s_cpu,
               sum(d_dr) FILTER (WHERE dt > 0 AND dt <= extract(epoch FROM c_max_gap)) AS s_dr,
               sum(d_dw) FILTER (WHERE dt > 0 AND dt <= extract(epoch FROM c_max_gap)) AS s_dw
        FROM d
        GROUP BY sb, b
    ),
    agg AS (
        SELECT b,
               sum(s_cpu::double precision / secs) / 1e9 AS cpu_cores,
               sum(mem_avg)::double precision AS mem_avg,
               sum(mem_max)::bigint AS mem_max,
               sum(sto_max)::bigint AS sto_max,
               sum(s_dr::double precision / secs) AS rd,
               sum(s_dw::double precision / secs) AS wr,
               sum(n)::integer AS n
        FROM per_sb
        GROUP BY b
    )
    SELECT g.g, a.cpu_cores, a.mem_avg, a.mem_max, a.sto_max, a.rd, a.wr, COALESCE(a.n, 0)
    FROM generate_series(p_from, p_to - interval '1 microsecond', p_bucket) AS g(g)
    LEFT JOIN agg a ON a.b = g.g
    ORDER BY g.g;
END;
$$;

-- Retention: delete at most p_limit samples older than p_older_than; returns how many. Callers
-- repeat while a full batch comes back.
CREATE FUNCTION workflows.prune_sandbox_samples(
    p_older_than interval DEFAULT interval '30 days', p_limit integer DEFAULT 10000
) RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, workflows, pg_temp
AS $$
DECLARE
    v_n bigint;
BEGIN
    IF p_older_than IS NULL OR p_limit IS NULL OR p_limit < 1 THEN
        RAISE EXCEPTION 'invalid retention arguments' USING ERRCODE = '22023';
    END IF;
    DELETE FROM workflows.sandbox_samples
    WHERE id IN (SELECT id FROM workflows.sandbox_samples
                 WHERE sampled_at < now() - p_older_than
                 ORDER BY sampled_at LIMIT p_limit);
    GET DIAGNOSTICS v_n = ROW_COUNT;
    RETURN v_n;
END;
$$;

REVOKE ALL ON FUNCTION workflows_api.record_sandbox_sample(text, bigint, text, timestamptz, bigint, bigint, bigint, bigint, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION workflows_api.sandbox_series(text, timestamptz, timestamptz, interval) FROM PUBLIC;
REVOKE ALL ON FUNCTION workflows.prune_sandbox_samples(interval, integer) FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT EXECUTE ON FUNCTION workflows_api.record_sandbox_sample(text, bigint, text, timestamptz, bigint, bigint, bigint, bigint, bigint) TO enzarb_workflows;
        GRANT EXECUTE ON FUNCTION workflows_api.sandbox_series(text, timestamptz, timestamptz, interval) TO enzarb_workflows;
        GRANT EXECUTE ON FUNCTION workflows.prune_sandbox_samples(interval, integer) TO enzarb_workflows;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
        GRANT USAGE ON SCHEMA workflows_api TO enzarb_ui;
        GRANT EXECUTE ON FUNCTION workflows_api.sandbox_series(text, timestamptz, timestamptz, interval) TO enzarb_ui;
    END IF;
END
$$;
