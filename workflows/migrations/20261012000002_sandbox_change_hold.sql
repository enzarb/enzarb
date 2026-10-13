-- Sandbox pod-shape changes (Project.status.pendingChange): while one is pending, no new agent
-- job of the Project is claimed, so the sandbox can drain. Additive only.
CREATE TABLE workflows.project_sandbox_changes (
    project_id    text        PRIMARY KEY,
    generation    bigint      NOT NULL,
    -- First time this pending episode was seen; the hold ceiling runs from here.
    first_seen_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at  timestamptz NOT NULL DEFAULT now()
);

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON workflows.project_sandbox_changes
            TO enzarb_workflows;
    ELSE
        RAISE NOTICE 'role enzarb_workflows missing; grants skipped';
    END IF;
END
$$;
