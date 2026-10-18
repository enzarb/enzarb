-- Failed inbox events are retried with backoff, not dropped (#335, ADR-0024 amendment).
-- Additive: `note` stays; a failing event keeps `processed_at` NULL.
ALTER TABLE engine.events
    ADD COLUMN attempts int NOT NULL DEFAULT 0,
    ADD COLUMN last_error text,
    ADD COLUMN retry_at timestamptz;
CREATE INDEX events_unprocessed_task ON engine.events (task_id, id) WHERE processed_at IS NULL;
CREATE INDEX events_unprocessed_subject ON engine.events (project, subject_key, id)
    WHERE processed_at IS NULL AND task_id IS NULL;

ALTER TABLE engine.findings DROP CONSTRAINT IF EXISTS findings_kind_check;
ALTER TABLE engine.findings ADD CONSTRAINT findings_kind_check
    CHECK (kind IN ('dead_wait', 'platform_failure', 'event_failing'));

-- One-time re-queue of events dropped by a state-loading failure (the #333 class). Bounded so a
-- stale command is never replayed: `error: state:` notes only, of tasks that have not ended,
-- received in the last 7 days. The matched ids are logged.
DO $$
DECLARE
    ids bigint[];
BEGIN
    WITH q AS (
        UPDATE engine.events e
        SET processed_at = NULL,
            last_error = substr(e.note, length('error: ') + 1),
            note = NULL,
            attempts = 0,
            retry_at = NULL
        FROM engine.tasks t
        WHERE t.id = e.task_id
          AND t.ended_at IS NULL
          AND e.processed_at IS NOT NULL
          AND e.note LIKE 'error: state:%'
          AND e.received_at >= now() - interval '7 days'
        RETURNING e.id
    )
    SELECT coalesce(array_agg(id ORDER BY id), '{}') INTO ids FROM q;
    RAISE NOTICE 'event_retry: re-queued % events: %', coalesce(array_length(ids, 1), 0), ids;
END
$$;
