-- Idempotent reconciler for enzarb_realtime's grants (the set 20261008000005 applies only if the
-- role existed at migrate time). pgop may create the role later, so the ticker calls this each pass.
-- The runtime role cannot GRANT on objects it does not own, hence SECURITY DEFINER: the function
-- runs as the migration owner, with a pinned search_path and no caller-supplied input.
-- Additive only. Returns true if any grant was made; false when the role is absent or nothing was missing.
CREATE FUNCTION workflows.reconcile_realtime_grants() RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $$
DECLARE
    granted boolean := false;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'enzarb_realtime') THEN
        RETURN false;
    END IF;

    -- Check before granting so a steady-state pass writes nothing (no ACL churn).
    IF NOT pg_catalog.has_database_privilege('enzarb_realtime', pg_catalog.current_database(), 'CONNECT') THEN
        EXECUTE pg_catalog.format('GRANT CONNECT ON DATABASE %I TO enzarb_realtime', pg_catalog.current_database());
        granted := true;
    END IF;
    IF NOT pg_catalog.has_schema_privilege('enzarb_realtime', 'workflows_api', 'USAGE') THEN
        GRANT USAGE ON SCHEMA workflows_api TO enzarb_realtime;
        granted := true;
    END IF;
    IF NOT pg_catalog.has_table_privilege('enzarb_realtime', 'workflows_api.work_item_project_v', 'SELECT') THEN
        GRANT SELECT ON workflows_api.work_item_project_v TO enzarb_realtime;
        granted := true;
    END IF;
    RETURN granted;
EXCEPTION
    WHEN serialization_failure THEN
        -- Another replica is granting at the same time; the next pass re-checks.
        RETURN false;
    WHEN OTHERS THEN
        IF SQLERRM LIKE 'tuple concurrently updated%' THEN
            RETURN false;
        END IF;
        RAISE;
END
$$;

REVOKE ALL ON FUNCTION workflows.reconcile_realtime_grants() FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT EXECUTE ON FUNCTION workflows.reconcile_realtime_grants() TO enzarb_workflows;
    ELSE
        RAISE NOTICE 'role enzarb_workflows missing; grants skipped';
    END IF;
END
$$;
