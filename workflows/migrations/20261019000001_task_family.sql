-- The task's place in an epic, for the board and the task page. Additive: `task_v` keeps its
-- columns and order and appends `parent` and `children`; old clients ignore them.
--
-- A relation row (T, parent, X) reads "T is parent of X"; (C, child, E) reads "C is child of E".
--   children: `[{key, task_id, title, status, end_status}]`, one per target of the task's `parent`
--             rows (or per task that names this one's subject in a `child` row); `task_id`, `title`
--             and the statuses are null while the target has no task of its own.
--   parent:   `{task_id, title, status, end_status}` of the task that is `parent` of one of this
--             task's subjects (or that this task names in a `child` row), else null.
-- Only tasks of the same project are resolved: the view reads every project, so a link to a task
-- of another project shows as a bare key (children) or not at all (parent).
CREATE OR REPLACE VIEW engine_api.task_v AS
SELECT p.task_id, p.project, t.repo, t.workflow_ref, p.title, p.subjects, p.status, p.end_status,
       p.phase, p.node, p."column", p.waiting_on,
       CASE WHEN COALESCE(p.reason, '') = '' THEN top.check_id ELSE p.reason END AS reason,
       CASE WHEN COALESCE(p.reason, '') = '' THEN COALESCE(NULLIF(p.text, ''), top.text) ELSE p.text END AS text,
       p.actions,
       COALESCE((SELECT jsonb_agg(a - 'content') FROM jsonb_array_elements(p.artifacts) a), '[]') AS artifacts,
       p.needs_you, p.cost_usd, t.created_at, p.updated_at, t.ended_at,
       COALESCE((SELECT jsonb_agg(jsonb_build_object(
                     'key', f.key, 'check', f.check_id, 'kind', f.kind, 'severity', f.severity,
                     'text', f.text, 'first_seen', f.first_seen, 'last_seen', f.last_seen)
                 ORDER BY f.severity_rank DESC, f.first_seen, f.key)
                FROM engine.findings f WHERE f.task_id = p.task_id AND f.cleared_at IS NULL), '[]') AS findings,
       p.commands,
       (SELECT jsonb_build_object('task_id', pt.task_id, 'title', pt.title, 'status', pt.status,
                                  'end_status', pt.end_status)
          FROM (SELECT r.task_id AS pid
                  FROM engine.subjects s
                  JOIN engine.relations r ON r.target_key = s.key AND r.kind = 'parent'
                 WHERE s.task_id = p.task_id
                UNION ALL
                SELECT CASE WHEN r.target_key ~ '^task:[0-9]+$' THEN substr(r.target_key, 6)::bigint
                            ELSE (SELECT s.task_id FROM engine.subjects s WHERE s.key = r.target_key
                                   ORDER BY s.ended, s.linked_at DESC LIMIT 1) END
                  FROM engine.relations r
                 WHERE r.task_id = p.task_id AND r.kind = 'child') c
          JOIN engine.projection pt ON pt.task_id = c.pid
         WHERE pt.task_id <> p.task_id AND pt.project = p.project
         ORDER BY pt.task_id LIMIT 1) AS parent,
       COALESCE((SELECT jsonb_agg(jsonb_build_object(
                     'key', d.key, 'task_id', d.task_id, 'title', d.title, 'status', d.status,
                     'end_status', d.end_status) ORDER BY d.key)
                FROM (SELECT DISTINCT ON (COALESCE(cp.task_id::text, c.key))
                             c.key, cp.task_id, cp.title, cp.status, cp.end_status
                        FROM (SELECT r.target_key AS key,
                                     (SELECT s.task_id FROM engine.subjects s WHERE s.key = r.target_key
                                       ORDER BY s.ended, s.linked_at DESC LIMIT 1) AS task_id
                                FROM engine.relations r WHERE r.task_id = p.task_id AND r.kind = 'parent'
                              UNION ALL
                              SELECT COALESCE((SELECT s.key FROM engine.subjects s WHERE s.task_id = r.task_id
                                                ORDER BY s.linked_at LIMIT 1), 'task:' || r.task_id),
                                     r.task_id
                                FROM engine.relations r
                               WHERE r.kind = 'child' AND r.target_key = 'task:' || p.task_id
                                 AND EXISTS (SELECT 1 FROM engine.projection x
                                              WHERE x.task_id = r.task_id AND x.project = p.project)
                              UNION ALL
                              SELECT COALESCE((SELECT s2.key FROM engine.subjects s2 WHERE s2.task_id = r.task_id
                                                ORDER BY s2.linked_at LIMIT 1), 'task:' || r.task_id),
                                     r.task_id
                                FROM engine.subjects s JOIN engine.relations r
                                     ON r.target_key = s.key AND r.kind = 'child'
                                 AND EXISTS (SELECT 1 FROM engine.projection x
                                              WHERE x.task_id = r.task_id AND x.project = p.project)
                               WHERE s.task_id = p.task_id) c
                        LEFT JOIN engine.projection cp ON cp.task_id = c.task_id AND cp.project = p.project
                       ORDER BY COALESCE(cp.task_id::text, c.key), c.key) d), '[]') AS children
FROM engine.projection p JOIN engine.tasks t ON t.id = p.task_id
LEFT JOIN LATERAL (
    SELECT f.check_id, f.text FROM engine.findings f
    WHERE f.task_id = p.task_id AND f.cleared_at IS NULL
    ORDER BY f.severity_rank DESC, f.first_seen, f.key LIMIT 1
) top ON true;

-- An epic's row carries its sub-issues' progress (`children`), so when a sub-issue's status or
-- title changes the epic is announced as changed too. Same channel and payload (a task id);
-- listeners already refetch the row by id.
CREATE OR REPLACE FUNCTION engine.notify_projection() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    epic bigint;
BEGIN
    PERFORM pg_notify('engine_projection', NEW.task_id::text);
    IF TG_OP = 'UPDATE' THEN
        IF OLD.status IS NOT DISTINCT FROM NEW.status
           AND OLD.end_status IS NOT DISTINCT FROM NEW.end_status
           AND OLD.title IS NOT DISTINCT FROM NEW.title THEN
            RETURN NEW;
        END IF;
    END IF;
    FOR epic IN
        SELECT r.task_id FROM engine.subjects s
          JOIN engine.relations r ON r.target_key = s.key AND r.kind = 'parent'
         WHERE s.task_id = NEW.task_id AND r.task_id <> NEW.task_id
        UNION
        SELECT substr(r.target_key, 6)::bigint FROM engine.relations r
         WHERE r.task_id = NEW.task_id AND r.kind = 'child' AND r.target_key ~ '^task:[0-9]+$'
    LOOP
        PERFORM pg_notify('engine_projection', epic::text);
    END LOOP;
    RETURN NEW;
END
$$;
