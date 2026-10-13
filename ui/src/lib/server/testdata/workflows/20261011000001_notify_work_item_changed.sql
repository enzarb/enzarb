-- NOTIFY work_item_changed (docs/contracts.md §6) from the database, so every writer is covered.
-- Postgres delivers notifications at commit and collapses identical (channel, payload) pairs
-- within a transaction: one notification per work item per committed transaction.
-- Payload stays the work item id. Safe to re-run.
--
-- Triggers fire only on changes the app_api/workflows_api views show. Lease-only, deadline-only
-- and updated_at-only job updates are excluded. When a view-visible column is added to
-- workflows.jobs or workflows.work_items, extend the UPDATE ... WHEN lists below in that migration.

CREATE OR REPLACE FUNCTION workflows.notify_work_item_changed() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM pg_notify('work_item_changed', OLD.work_item_id::text);
        RETURN OLD;
    END IF;
    IF TG_TABLE_NAME = 'work_items' THEN
        PERFORM pg_notify('work_item_changed', NEW.id::text);
        RETURN NEW;
    END IF;
    -- transitions.work_item_id is nullable (audit rows for unmatched events): nothing to wake.
    IF NEW.work_item_id IS NULL THEN
        RETURN NEW;
    END IF;
    PERFORM pg_notify('work_item_changed', NEW.work_item_id::text);
    IF TG_OP = 'UPDATE' AND OLD.work_item_id IS DISTINCT FROM NEW.work_item_id THEN
        PERFORM pg_notify('work_item_changed', OLD.work_item_id::text);
    END IF;
    RETURN NEW;
END
$$;

DROP TRIGGER IF EXISTS jobs_notify_ins ON workflows.jobs;
CREATE TRIGGER jobs_notify_ins AFTER INSERT OR DELETE ON workflows.jobs
    FOR EACH ROW EXECUTE FUNCTION workflows.notify_work_item_changed();

DROP TRIGGER IF EXISTS jobs_notify_upd ON workflows.jobs;
CREATE TRIGGER jobs_notify_upd AFTER UPDATE ON workflows.jobs
    FOR EACH ROW
    WHEN (OLD.status IS DISTINCT FROM NEW.status
       OR OLD.attempt IS DISTINCT FROM NEW.attempt
       OR OLD.run_after IS DISTINCT FROM NEW.run_after
       OR OLD.output IS DISTINCT FROM NEW.output
       OR OLD.work_item_id IS DISTINCT FROM NEW.work_item_id)
    EXECUTE FUNCTION workflows.notify_work_item_changed();

DROP TRIGGER IF EXISTS work_items_notify_ins ON workflows.work_items;
CREATE TRIGGER work_items_notify_ins AFTER INSERT ON workflows.work_items
    FOR EACH ROW EXECUTE FUNCTION workflows.notify_work_item_changed();

DROP TRIGGER IF EXISTS work_items_notify_upd ON workflows.work_items;
CREATE TRIGGER work_items_notify_upd AFTER UPDATE ON workflows.work_items
    FOR EACH ROW
    WHEN (OLD.state IS DISTINCT FROM NEW.state
       OR OLD.state_data IS DISTINCT FROM NEW.state_data
       OR OLD.project_id IS DISTINCT FROM NEW.project_id)
    EXECUTE FUNCTION workflows.notify_work_item_changed();

DROP TRIGGER IF EXISTS transitions_notify ON workflows.transitions;
CREATE TRIGGER transitions_notify AFTER INSERT ON workflows.transitions
    FOR EACH ROW EXECUTE FUNCTION workflows.notify_work_item_changed();
