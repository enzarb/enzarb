-- Runtime role for the engine itself: read/write on its own schema, now and for future tables.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT USAGE ON SCHEMA workflows, workflows_api TO enzarb_workflows;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA workflows TO enzarb_workflows;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA workflows TO enzarb_workflows;
        ALTER DEFAULT PRIVILEGES IN SCHEMA workflows
            GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO enzarb_workflows;
        ALTER DEFAULT PRIVILEGES IN SCHEMA workflows
            GRANT USAGE, SELECT ON SEQUENCES TO enzarb_workflows;
    ELSE
        RAISE NOTICE 'role enzarb_workflows missing; grants skipped';
    END IF;
END
$$;
