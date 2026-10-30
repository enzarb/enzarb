-- Runs started by hand (`POST /v1/runs`) have no connector event. They get a negative, unique
-- `event_id` from this sequence so (event_id, workflow, test) stays unique and never collides
-- with `connectors.events.id`.
CREATE SEQUENCE tracks.manual_event_seq;
