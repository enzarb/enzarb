-- Epics (docs/adr/0018-epics.md). Additive: one more dependency source.
--
-- `epic`  the structure enzarb records when it creates the sub-issues of an approved epic: the
--         epic waits for each sub-issue, and a sub-issue waits for the sub-issues the plan
--         orders before it. The engine writes these itself; nothing syncs or edits them.
ALTER TABLE workflows.work_item_dependencies
    DROP CONSTRAINT work_item_dependencies_source_check,
    ADD CONSTRAINT work_item_dependencies_source_check
        CHECK (source IN ('github', 'text', 'comment', 'epic'));
