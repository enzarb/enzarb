-- ADR-0022: work item ids now name engine v2 tasks, so entries recorded for the old engine's work
-- items would match unrelated tasks. Drop them; the notifier rebuilds entries as tasks change.
DELETE FROM app.push_item;
