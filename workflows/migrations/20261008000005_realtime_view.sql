-- Narrow view for enzarb-realtime: resolves a work item id to its project, nothing else.
-- Same source as work_item_v; owner-run like the other _v views.
CREATE VIEW workflows_api.work_item_project_v AS
SELECT id, project_id AS project FROM workflows.work_items;

-- enzarb_realtime is created by pgop in prod; absent roles (bare local DBs) are skipped with a notice.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_realtime') THEN
        EXECUTE format('GRANT CONNECT ON DATABASE %I TO enzarb_realtime', current_database());
        GRANT USAGE ON SCHEMA workflows_api TO enzarb_realtime;
        GRANT SELECT ON workflows_api.work_item_project_v TO enzarb_realtime;
    ELSE
        RAISE NOTICE 'role enzarb_realtime missing; grants skipped';
    END IF;
END
$$;
