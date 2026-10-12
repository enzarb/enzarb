-- Engine source of truth. Owned by enzarb-workflows; nobody else touches these tables.
CREATE SCHEMA IF NOT EXISTS workflows;

-- Inbox: every input to the engine.
CREATE TABLE workflows.events (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source        text        NOT NULL,
    dedupe_key    text        NOT NULL UNIQUE,
    work_item_key text        NOT NULL,
    type          text        NOT NULL,
    payload       jsonb       NOT NULL DEFAULT '{}'::jsonb,
    received_at   timestamptz NOT NULL DEFAULT now(),
    processed_at  timestamptz
);
-- Reducer claim: oldest unprocessed event.
CREATE INDEX events_unprocessed_idx ON workflows.events (id) WHERE processed_at IS NULL;
CREATE INDEX events_work_item_key_idx ON workflows.events (work_item_key, id);

-- One row per issue / PR / mention task.
CREATE TABLE workflows.work_items (
    id               bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    key              text        NOT NULL UNIQUE,
    project_id       text,
    workflow         text        NOT NULL,
    workflow_version integer     NOT NULL DEFAULT 1,
    state            text        NOT NULL,
    state_data       jsonb       NOT NULL DEFAULT '{}'::jsonb,
    wake_at          timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
);
-- Ticker: work items whose timer is due.
CREATE INDEX work_items_wake_idx ON workflows.work_items (wake_at) WHERE wake_at IS NOT NULL;
CREATE INDEX work_items_project_idx ON workflows.work_items (project_id);

-- Append-only history; feeds UI checklists and audit.
CREATE TABLE workflows.transitions (
    id           bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    work_item_id bigint      NOT NULL REFERENCES workflows.work_items (id),
    from_state   text,
    to_state     text        NOT NULL,
    event_id     bigint REFERENCES workflows.events (id),
    event_type   text        NOT NULL,
    effects      jsonb       NOT NULL DEFAULT '[]'::jsonb,
    at           timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX transitions_work_item_idx ON workflows.transitions (work_item_id, id);
CREATE INDEX transitions_event_idx ON workflows.transitions (event_id);

-- Effects waiting to run, running, or done.
CREATE TABLE workflows.jobs (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    work_item_id    bigint      NOT NULL REFERENCES workflows.work_items (id),
    project_id      text,
    kind            text        NOT NULL,
    priority        integer     NOT NULL DEFAULT 0,
    idempotency_key text        NOT NULL UNIQUE,
    status          text        NOT NULL DEFAULT 'queued'
                    CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled')),
    attempt         integer     NOT NULL DEFAULT 0,
    max_attempts    integer     NOT NULL DEFAULT 5,
    run_after       timestamptz NOT NULL DEFAULT now(),
    lease_owner     text,
    lease_until     timestamptz,
    -- Running agent jobs release their lease but keep a deadline; the ticker turns expiry into events.
    deadline        timestamptz,
    input           jsonb       NOT NULL DEFAULT '{}'::jsonb,
    output          jsonb,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
-- Executor claim: ORDER BY priority DESC, run_after, id among queued jobs.
CREATE INDEX jobs_claim_idx ON workflows.jobs (priority DESC, run_after, id) WHERE status = 'queued';
-- Concurrency caps count running jobs per project (and kind).
CREATE INDEX jobs_running_project_idx ON workflows.jobs (project_id, kind) WHERE status = 'running';
-- Crash recovery: running jobs whose lease expired.
CREATE INDEX jobs_lease_idx ON workflows.jobs (lease_until) WHERE status = 'running' AND lease_until IS NOT NULL;
-- Ticker: running jobs past their deadline.
CREATE INDEX jobs_deadline_idx ON workflows.jobs (deadline) WHERE status = 'running' AND deadline IS NOT NULL;
CREATE INDEX jobs_work_item_idx ON workflows.jobs (work_item_id, id);
