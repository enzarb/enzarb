-- ADR-0022 engine storage (docs/workflows/contracts-v2.md). Schema `engine` is owned by
-- workflows; `state` and `resolved` are opaque to everything but the engine.
CREATE SCHEMA IF NOT EXISTS engine;
CREATE SCHEMA IF NOT EXISTS engine_api;

CREATE TABLE engine.tasks (
    id bigserial PRIMARY KEY,
    project text NOT NULL,
    repo text NOT NULL,
    binding text,
    workflow_ref text NOT NULL,
    state jsonb NOT NULL,
    resolved jsonb NOT NULL,
    origins jsonb NOT NULL DEFAULT '[]',
    -- Login of the actor whose event started the task (user-scope secrets).
    started_by text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    ended_at timestamptz
);
CREATE INDEX tasks_project ON engine.tasks (project, updated_at DESC);

CREATE TABLE engine.subjects (
    task_id bigint NOT NULL REFERENCES engine.tasks (id) ON DELETE CASCADE,
    kind text NOT NULL,
    key text NOT NULL,
    url text,
    linked_at timestamptz NOT NULL DEFAULT now(),
    -- Mirrors tasks.ended_at IS NOT NULL (kept in sync by the engine), for the partial index.
    ended boolean NOT NULL DEFAULT false,
    PRIMARY KEY (task_id, key)
);
-- An object belongs to at most one live task.
CREATE UNIQUE INDEX subjects_live_key ON engine.subjects (key) WHERE NOT ended;
CREATE INDEX subjects_key ON engine.subjects (key);

CREATE TABLE engine.relations (
    task_id bigint NOT NULL REFERENCES engine.tasks (id) ON DELETE CASCADE,
    kind text NOT NULL,
    target_key text NOT NULL,
    source text NOT NULL DEFAULT 'workflow',
    PRIMARY KEY (task_id, kind, target_key)
);
CREATE INDEX relations_target ON engine.relations (target_key);

CREATE TABLE engine.events (
    id bigserial PRIMARY KEY,
    dedupe_key text NOT NULL UNIQUE,
    project text,
    subject_key text,
    task_id bigint REFERENCES engine.tasks (id) ON DELETE SET NULL,
    type text NOT NULL,
    payload jsonb NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    processed_at timestamptz,
    -- Why the event did nothing (unrouted, no binding, error), for the inspector.
    note text
);
CREATE INDEX events_unprocessed ON engine.events (id) WHERE processed_at IS NULL;
CREATE INDEX events_task ON engine.events (task_id, id);

CREATE TABLE engine.transitions (
    id bigserial PRIMARY KEY,
    task_id bigint NOT NULL REFERENCES engine.tasks (id) ON DELETE CASCADE,
    event_id bigint REFERENCES engine.events (id) ON DELETE SET NULL,
    from_state jsonb NOT NULL,
    to_state jsonb NOT NULL,
    effects jsonb NOT NULL,
    at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX transitions_task ON engine.transitions (task_id, id);

CREATE TABLE engine.jobs (
    id bigserial PRIMARY KEY,
    task_id bigint NOT NULL REFERENCES engine.tasks (id) ON DELETE CASCADE,
    project text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('run_step', 'steer', 'stop_step', 'call_action', 'refresh_fact')),
    input jsonb NOT NULL,
    -- queued, running, done, failed, blocked (waits on a human: missing secret)
    status text NOT NULL DEFAULT 'queued',
    attempt int NOT NULL DEFAULT 0,
    run_after timestamptz NOT NULL DEFAULT now(),
    lease_owner text,
    lease_until timestamptz,
    output jsonb,
    error text,
    idempotency_key text NOT NULL UNIQUE,
    priority int NOT NULL DEFAULT 0,
    concurrency_group text,
    concurrency_max int,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX jobs_ready ON engine.jobs (priority DESC, run_after, id) WHERE status IN ('queued', 'running');

CREATE TABLE engine.timers (
    task_id bigint NOT NULL REFERENCES engine.tasks (id) ON DELETE CASCADE,
    timer_id text NOT NULL,
    fire_at timestamptz NOT NULL,
    fired boolean NOT NULL DEFAULT false,
    PRIMARY KEY (task_id, timer_id)
);
CREATE INDEX timers_due ON engine.timers (fire_at) WHERE NOT fired;

CREATE TABLE engine.step_runs (
    run_id text PRIMARY KEY,
    task_id bigint NOT NULL REFERENCES engine.tasks (id) ON DELETE CASCADE,
    project text NOT NULL,
    node text NOT NULL,
    step text NOT NULL,
    attempt int NOT NULL DEFAULT 1,
    runtime text,
    model text,
    queued_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    ended_at timestamptz,
    last_activity_at timestamptz,
    active_secs double precision NOT NULL DEFAULT 0,
    queue_secs double precision NOT NULL DEFAULT 0,
    exit text,
    tokens jsonb,
    cost_usd numeric,
    resumes int NOT NULL DEFAULT 0,
    stalls int NOT NULL DEFAULT 0,
    -- The quiet period that was already reported as stalled (no repeat until new activity).
    stall_reported_at timestamptz,
    steers int NOT NULL DEFAULT 0,
    sandbox jsonb
);
CREATE INDEX step_runs_task ON engine.step_runs (task_id);
CREATE INDEX step_runs_running ON engine.step_runs (project) WHERE ended_at IS NULL;

-- Partitioned by month on `at`; PK includes the partition key.
CREATE TABLE engine.session_events (
    run_id text NOT NULL,
    seq int NOT NULL,
    at timestamptz NOT NULL,
    kind text NOT NULL,
    payload jsonb NOT NULL,
    truncated boolean NOT NULL DEFAULT false,
    PRIMARY KEY (run_id, seq, at)
) PARTITION BY RANGE (at);
CREATE INDEX session_events_run ON engine.session_events (run_id, seq);
CREATE INDEX session_events_payload ON engine.session_events USING gin (payload jsonb_path_ops);
CREATE TABLE engine.session_events_default PARTITION OF engine.session_events DEFAULT;

-- Creates the monthly partition containing `month` (idempotent). Rows already in the default
-- partition for that month block the creation; the caller then just keeps using the default.
CREATE FUNCTION engine.ensure_session_partition(month date) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    lo date := date_trunc('month', month)::date;
    hi date := (date_trunc('month', month) + interval '1 month')::date;
    name text := 'session_events_' || to_char(lo, 'YYYYMM');
BEGIN
    IF to_regclass('engine.' || name) IS NULL THEN
        EXECUTE format('CREATE TABLE engine.%I PARTITION OF engine.session_events FOR VALUES FROM (%L) TO (%L)', name, lo, hi);
    END IF;
EXCEPTION WHEN check_violation THEN
    RAISE NOTICE 'partition % overlaps rows in the default partition; skipped', name;
END
$$;
SELECT engine.ensure_session_partition(now()::date);
SELECT engine.ensure_session_partition((now() + interval '1 month')::date);

CREATE TABLE engine.facts (
    task_id bigint NOT NULL REFERENCES engine.tasks (id) ON DELETE CASCADE,
    name text NOT NULL,
    value jsonb NOT NULL,
    source text NOT NULL,
    at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (task_id, name)
);

-- Named secrets. `ciphertext` = 12-byte nonce || AES-256-GCM(ciphertext+tag) with the key from
-- ENZARB_SECRETS_KEY. `engine_api.secret_set` (no key in the database) writes `pending`; the
-- engine encrypts it on its next tick and clears `pending`.
CREATE TABLE engine.secrets (
    scope text NOT NULL CHECK (scope IN ('user', 'project', 'instance')),
    owner text NOT NULL DEFAULT '',
    name text NOT NULL,
    ciphertext bytea,
    pending text,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (scope, owner, name)
);

CREATE TABLE engine.projection (
    task_id bigint PRIMARY KEY REFERENCES engine.tasks (id) ON DELETE CASCADE,
    project text NOT NULL,
    repo text NOT NULL,
    workflow text NOT NULL,
    title text NOT NULL DEFAULT '',
    subjects jsonb NOT NULL DEFAULT '[]',
    status text NOT NULL,
    end_status text,
    phase text NOT NULL DEFAULT '',
    node text NOT NULL DEFAULT '',
    "column" text,
    waiting_on jsonb,
    reason text,
    text text,
    actions jsonb NOT NULL DEFAULT '[]',
    artifacts jsonb NOT NULL DEFAULT '[]',
    needs_you boolean NOT NULL DEFAULT false,
    cost_usd numeric NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX projection_project ON engine.projection (project, updated_at DESC);
CREATE INDEX projection_needs_you ON engine.projection (updated_at DESC) WHERE needs_you;

-- What the engine last resolved per Project/repo, for the board and the read-only viewer.
CREATE TABLE engine.resolutions (
    project text NOT NULL,
    repo text NOT NULL,
    board jsonb NOT NULL DEFAULT '{}',
    bindings jsonb NOT NULL DEFAULT '[]',
    workflows jsonb NOT NULL DEFAULT '{}',
    errors jsonb NOT NULL DEFAULT '[]',
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project, repo)
);

-- Chats (ADR-0014) are not tasks; their sessions use session_events with run_id `chat-<id>`.
CREATE TABLE engine.chats (
    run_id text PRIMARY KEY CHECK (run_id LIKE 'chat-%'),
    project text NOT NULL,
    "user" text NOT NULL,
    title text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- Sandbox utilization samples (ADR-0019), attributed to the step runs running at the time.
CREATE TABLE engine.sandbox_stats (
    project text NOT NULL,
    sandbox text NOT NULL,
    at timestamptz NOT NULL,
    run_ids text[] NOT NULL DEFAULT '{}',
    cpu_usage_usec bigint,
    cpu_user_usec bigint,
    cpu_system_usec bigint,
    memory_bytes bigint,
    io_read_bytes bigint,
    io_write_bytes bigint,
    storage_used_bytes bigint,
    storage_total_bytes bigint,
    PRIMARY KEY (project, sandbox, at)
);

-- Sandbox drain holds while the operator waits to roll a Project's sandbox.
CREATE TABLE engine.sandbox_holds (
    project text PRIMARY KEY,
    generation bigint NOT NULL,
    since timestamptz NOT NULL DEFAULT now()
);

-- NOTIFY: the UI and realtime listen on these.
CREATE FUNCTION engine.notify_projection() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('engine_projection', NEW.task_id::text);
    RETURN NEW;
END
$$;
CREATE TRIGGER projection_notify AFTER INSERT OR UPDATE ON engine.projection
    FOR EACH ROW EXECUTE FUNCTION engine.notify_projection();

CREATE FUNCTION engine.notify_event() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('engine_events', NEW.id::text);
    RETURN NEW;
END
$$;
CREATE TRIGGER events_notify AFTER INSERT ON engine.events
    FOR EACH ROW EXECUTE FUNCTION engine.notify_event();

CREATE FUNCTION engine.notify_session() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('engine_session', NEW.run_id || ':' || NEW.seq);
    RETURN NEW;
END
$$;
CREATE TRIGGER session_notify AFTER INSERT ON engine.session_events
    FOR EACH ROW EXECUTE FUNCTION engine.notify_session();
