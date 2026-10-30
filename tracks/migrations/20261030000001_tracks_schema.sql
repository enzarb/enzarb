-- Schema `tracks` (owner: enzarb_tracks_owner). History lives in tracks._sqlx_migrations.
-- pgop creates this schema; only create it when it is missing (see enzarb-tracks db.rs).
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'tracks') THEN
        CREATE SCHEMA tracks;
    END IF;
END $$;

-- One row per connector event tracks routed (ADR-0029 section 5). The primary key is what makes
-- routing at-most-once: a second writer for the same event conflicts and does nothing.
CREATE TABLE tracks.event_decisions (
    event_id          bigint PRIMARY KEY,
    -- Lowercase owner/repo; NULL for deliveries that name no repository.
    repo              text,
    -- The owning Project, NULL when none was found.
    project           text,
    -- Default-branch head the workflow files were read at (pinned for the runs).
    sha               text,
    -- started | no_match | skipped | error
    outcome           text        NOT NULL CHECK (outcome IN ('started', 'no_match', 'skipped', 'error')),
    -- Ids of the workflows that matched and started a run (empty when none).
    matched           jsonb       NOT NULL DEFAULT '[]'::jsonb,
    -- [{workflow, matched, on, reason}] for every workflow in the file (the on / if evaluation).
    reasons           jsonb       NOT NULL DEFAULT '[]'::jsonb,
    -- Human-readable problems: invalid file, missing project, GitHub errors.
    errors            jsonb       NOT NULL DEFAULT '[]'::jsonb,
    event_received_at timestamptz,
    decided_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX event_decisions_repo_idx ON tracks.event_decisions (repo, event_id DESC);

-- Where the catch-up starts: events at or below `floor` are never routed (history from before
-- tracks was first deployed). Single row.
CREATE TABLE tracks.router_state (
    id         boolean PRIMARY KEY DEFAULT true CHECK (id),
    floor      bigint      NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- A workflow run (ADR-0029 section 5). Step 3 only creates rows in `pending`; nothing executes.
CREATE TABLE tracks.runs (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workflow     text        NOT NULL,
    project      text        NOT NULL,
    repo         text        NOT NULL,
    -- `owner/repo#number` of the issue or pull request, NULL for events without one.
    subject_key  text,
    subject      jsonb,
    event_id     bigint      NOT NULL,
    -- Default-branch commit the workflow and modes were read at; pinned for the run's life.
    sha          text        NOT NULL,
    -- pending | running | waiting | done | failed | cancelled
    status       text        NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'running', 'waiting', 'done', 'failed', 'cancelled')),
    test         boolean     NOT NULL DEFAULT false,
    current_node text,
    visits       jsonb       NOT NULL DEFAULT '{}'::jsonb,
    -- ez-workflow RunState as of the last transition.
    state        jsonb       NOT NULL,
    reason       text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    finished_at  timestamptz,
    -- An event starts a workflow at most once.
    UNIQUE (event_id, workflow, test)
);
CREATE INDEX runs_subject_idx ON tracks.runs (subject_key, workflow) WHERE finished_at IS NULL;
CREATE INDEX runs_project_idx ON tracks.runs (project, created_at DESC);

-- One row per node visit.
CREATE TABLE tracks.node_runs (
    id          bigserial PRIMARY KEY,
    run_id      uuid        NOT NULL REFERENCES tracks.runs (id) ON DELETE CASCADE,
    node        text        NOT NULL,
    visit       integer     NOT NULL,
    status      text        NOT NULL DEFAULT 'pending',
    inputs      jsonb,
    outputs     jsonb,
    usage       jsonb,
    error       text,
    started_at  timestamptz,
    finished_at timestamptz,
    UNIQUE (run_id, node, visit)
);

-- Runtime role is created by pgop in prod; absent roles (bare local DBs) are skipped.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_tracks') THEN
        GRANT USAGE ON SCHEMA tracks TO enzarb_tracks;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA tracks TO enzarb_tracks;
        GRANT USAGE ON ALL SEQUENCES IN SCHEMA tracks TO enzarb_tracks;
        ALTER DEFAULT PRIVILEGES IN SCHEMA tracks
            GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO enzarb_tracks;
        ALTER DEFAULT PRIVILEGES IN SCHEMA tracks GRANT USAGE ON SEQUENCES TO enzarb_tracks;
    ELSE
        RAISE NOTICE 'role enzarb_tracks missing; grants skipped';
    END IF;
END
$$;
