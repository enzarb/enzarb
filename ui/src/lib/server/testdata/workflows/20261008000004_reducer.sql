-- Reducer support. Additive.

-- Events that match no Project (or cannot start a work item) are audited by a transition
-- row that has no work item, so work_item_id becomes nullable. `note` records why a
-- transition was rejected or an event ignored.
ALTER TABLE workflows.transitions ALTER COLUMN work_item_id DROP NOT NULL;
ALTER TABLE workflows.transitions ADD COLUMN note text;

-- The engine's own ticker and job-result helper ingest through the public function.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT EXECUTE ON FUNCTION workflows_api.ingest_event(text, text, text, text, jsonb) TO enzarb_workflows;
    END IF;
END
$$;
