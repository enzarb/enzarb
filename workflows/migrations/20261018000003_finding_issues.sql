-- A finding's check may file one issue (ADR-0024 addendum): `file_issue` jobs, keyed
-- `finding:<check>:<subject>`. Platform-scope findings have no task, so `task_id` may be NULL.
-- Additive: every existing job keeps its kind and task.
ALTER TABLE engine.jobs ALTER COLUMN task_id DROP NOT NULL;
ALTER TABLE engine.jobs DROP CONSTRAINT jobs_kind_check;
ALTER TABLE engine.jobs ADD CONSTRAINT jobs_kind_check
    CHECK (kind IN ('run_step', 'steer', 'stop_step', 'call_action', 'refresh_fact', 'file_issue'));
