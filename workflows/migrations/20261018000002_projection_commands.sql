-- The commands that apply to the task now (`[{name, label, description, from}]`), written by the
-- reducer next to `actions` (docs/contracts.md, workflows_api). Additive: old clients ignore it.
ALTER TABLE engine.projection ADD COLUMN commands jsonb NOT NULL DEFAULT '[]';

-- Same columns in the same order, `commands` appended.
CREATE OR REPLACE VIEW engine_api.task_v AS
SELECT p.task_id, p.project, t.repo, t.workflow_ref, p.title, p.subjects, p.status, p.end_status,
       p.phase, p.node, p."column", p.waiting_on,
       CASE WHEN COALESCE(p.reason, '') = '' THEN top.check_id ELSE p.reason END AS reason,
       CASE WHEN COALESCE(p.reason, '') = '' THEN COALESCE(NULLIF(p.text, ''), top.text) ELSE p.text END AS text,
       p.actions,
       -- artifacts without content (task_detail has the content)
       COALESCE((SELECT jsonb_agg(a - 'content') FROM jsonb_array_elements(p.artifacts) a), '[]') AS artifacts,
       p.needs_you, p.cost_usd, t.created_at, p.updated_at, t.ended_at,
       COALESCE((SELECT jsonb_agg(jsonb_build_object(
                     'key', f.key, 'check', f.check_id, 'kind', f.kind, 'severity', f.severity,
                     'text', f.text, 'first_seen', f.first_seen, 'last_seen', f.last_seen)
                 ORDER BY f.severity_rank DESC, f.first_seen, f.key)
                FROM engine.findings f WHERE f.task_id = p.task_id AND f.cleared_at IS NULL), '[]') AS findings,
       p.commands
FROM engine.projection p JOIN engine.tasks t ON t.id = p.task_id
LEFT JOIN LATERAL (
    SELECT f.check_id, f.text FROM engine.findings f
    WHERE f.task_id = p.task_id AND f.cleared_at IS NULL
    ORDER BY f.severity_rank DESC, f.first_seen, f.key LIMIT 1
) top ON true;
