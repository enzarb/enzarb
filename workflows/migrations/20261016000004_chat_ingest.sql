-- Chat ingestion (ADR-0014 chats are not tasks): sandbox-proxy, which relays the browser's ACP
-- channel, records each chat as engine.chats plus session_events of run `chat-<session id>`.
CREATE FUNCTION engine_api.chat_record(p_run_id text, p_project text, p_user text, p_title text)
RETURNS void
LANGUAGE sql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
INSERT INTO chats (run_id, project, "user", title) VALUES (p_run_id, p_project, p_user, left(coalesce(p_title, ''), 200))
ON CONFLICT (run_id) DO UPDATE SET title = CASE WHEN chats.title = '' THEN EXCLUDED.title ELSE chats.title END
$$;

-- Appends one session event (seq = next per run). Payloads over 1 MB keep head and tail.
CREATE FUNCTION engine_api.chat_event(p_run_id text, p_kind text, p_payload jsonb)
RETURNS int
LANGUAGE plpgsql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
DECLARE
    n int;
    t text := p_payload::text;
    big boolean := octet_length(p_payload::text) > 1048576;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM chats WHERE run_id = p_run_id) THEN
        RAISE EXCEPTION 'unknown chat %', p_run_id;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtext(p_run_id));
    SELECT coalesce(max(seq), 0) + 1 INTO n FROM session_events WHERE run_id = p_run_id;
    INSERT INTO session_events (run_id, seq, at, kind, payload, truncated)
    VALUES (p_run_id, n, now(), p_kind,
            CASE WHEN big THEN jsonb_build_object('head', left(t, 524288), 'tail', right(t, 524288), 'bytes', octet_length(t)) ELSE p_payload END,
            big);
    RETURN n;
END
$$;

REVOKE ALL ON FUNCTION engine_api.chat_record(text, text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION engine_api.chat_event(text, text, jsonb) FROM PUBLIC;

CREATE OR REPLACE FUNCTION engine.apply_grants() RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path = engine, pg_temp AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_workflows') THEN
        GRANT USAGE ON SCHEMA engine, engine_api TO enzarb_workflows;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA engine TO enzarb_workflows;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA engine TO enzarb_workflows;
        GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA engine TO enzarb_workflows;
        GRANT SELECT ON ALL TABLES IN SCHEMA engine_api TO enzarb_workflows;
        GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA engine_api TO enzarb_workflows;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
        GRANT USAGE ON SCHEMA engine_api TO enzarb_ui;
        GRANT SELECT ON engine_api.task_v TO enzarb_ui;
        GRANT EXECUTE ON FUNCTION
            engine_api.task_detail(bigint), engine_api.session_v(text, int, int),
            engine_api.session_search(text, text), engine_api.board_v(text),
            engine_api.workflow_v(text, text), engine_api.metrics(text, timestamptz, timestamptz),
            engine_api.submit(bigint, jsonb, text, jsonb),
            engine_api.secret_set(text, text, text, text), engine_api.secret_delete(text, text, text),
            engine_api.secret_list(text, text), engine_api.chat_v(text)
        TO enzarb_ui;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_connectors') THEN
        GRANT USAGE ON SCHEMA engine_api TO enzarb_connectors;
        GRANT EXECUTE ON FUNCTION engine_api.ingest(text, text, text, text, jsonb),
            engine_api.secret_for_receiver(text, text) TO enzarb_connectors;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_realtime') THEN
        EXECUTE format('GRANT CONNECT ON DATABASE %I TO enzarb_realtime', current_database());
        GRANT USAGE ON SCHEMA engine_api TO enzarb_realtime;
        GRANT SELECT ON engine_api.task_v TO enzarb_realtime;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_sandbox_proxy') THEN
        GRANT USAGE ON SCHEMA engine_api TO enzarb_sandbox_proxy;
        GRANT EXECUTE ON FUNCTION engine_api.chat_record(text, text, text, text),
            engine_api.chat_event(text, text, jsonb) TO enzarb_sandbox_proxy;
    END IF;
END
$$;
SELECT engine.apply_grants();
