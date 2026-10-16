-- Local test fixture for the engine v2 `engine` / `engine_api` schemas (docs/workflows/contracts-v2.md).
-- TODO(M7 integration): replace with the real workflows/migrations once the engine core branch lands;
-- the shapes below are what ui/src/lib/server/tasks-service.ts reads, and the UI-side assumptions
-- that go beyond contracts-v2 are listed in testdata/README.md.

CREATE SCHEMA engine;
CREATE SCHEMA engine_api;

CREATE TABLE engine.tasks (
	id bigserial PRIMARY KEY,
	project text NOT NULL,
	repo text,
	binding text,
	workflow_ref text,
	state jsonb NOT NULL DEFAULT '{}',
	resolved jsonb NOT NULL DEFAULT '{}',
	origins jsonb NOT NULL DEFAULT '{}',
	created_at timestamptz NOT NULL DEFAULT now(),
	updated_at timestamptz NOT NULL DEFAULT now(),
	ended_at timestamptz
);

CREATE TABLE engine.events (
	id bigserial PRIMARY KEY,
	dedupe_key text UNIQUE NOT NULL,
	project text,
	subject_key text,
	task_id bigint,
	type text NOT NULL,
	payload jsonb NOT NULL,
	received_at timestamptz NOT NULL DEFAULT now(),
	processed_at timestamptz
);

CREATE TABLE engine.transitions (
	id bigserial PRIMARY KEY,
	task_id bigint NOT NULL,
	event_id bigint,
	"from" jsonb,
	"to" jsonb,
	effects jsonb NOT NULL DEFAULT '[]',
	at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE engine.step_runs (
	run_id text PRIMARY KEY,
	task_id bigint NOT NULL,
	node text NOT NULL,
	step text NOT NULL,
	attempt int NOT NULL DEFAULT 1,
	runtime text,
	model text,
	started_at timestamptz,
	ended_at timestamptz,
	active_secs double precision,
	queue_secs double precision,
	exit text,
	tokens jsonb,
	cost_usd numeric,
	resumes int NOT NULL DEFAULT 0,
	stalls int NOT NULL DEFAULT 0,
	steers int NOT NULL DEFAULT 0,
	sandbox jsonb
);

CREATE TABLE engine.session_events (
	run_id text NOT NULL,
	seq int NOT NULL,
	at timestamptz NOT NULL DEFAULT now(),
	kind text NOT NULL,
	payload jsonb NOT NULL,
	truncated boolean NOT NULL DEFAULT false,
	PRIMARY KEY (run_id, seq)
);

CREATE TABLE engine.secrets (
	scope text NOT NULL,
	owner text NOT NULL,
	name text NOT NULL,
	ciphertext bytea NOT NULL,
	updated_at timestamptz NOT NULL DEFAULT now(),
	PRIMARY KEY (scope, owner, name)
);

CREATE TABLE engine.projection (
	task_id bigint PRIMARY KEY,
	project text NOT NULL,
	title text NOT NULL,
	subjects jsonb NOT NULL DEFAULT '[]',
	status text NOT NULL,
	end_status text,
	phase text NOT NULL DEFAULT '',
	node text NOT NULL DEFAULT '',
	"column" text,
	waiting_on text,
	reason text,
	text text,
	actions jsonb NOT NULL DEFAULT '[]',
	artifacts jsonb NOT NULL DEFAULT '[]',
	needs_you boolean NOT NULL DEFAULT false,
	cost_usd numeric,
	updated_at timestamptz NOT NULL DEFAULT now()
);

-- Fixture-only stand-ins for the engine's resolver output (the real engine resolves from git).
CREATE TABLE engine.boards (project text PRIMARY KEY, board jsonb NOT NULL);
CREATE TABLE engine.workflow_views (project text NOT NULL, repo text NOT NULL, view jsonb NOT NULL,
	PRIMARY KEY (project, repo));
CREATE TABLE engine.chats (run_id text PRIMARY KEY, "user" text NOT NULL, project text NOT NULL,
	title text, started_at timestamptz NOT NULL DEFAULT now());

CREATE FUNCTION engine.projection_notify() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
	PERFORM pg_notify('engine_projection', NEW.task_id::text);
	RETURN NEW;
END $$;
CREATE TRIGGER projection_notify AFTER INSERT OR UPDATE ON engine.projection
	FOR EACH ROW EXECUTE FUNCTION engine.projection_notify();

-- ------------------------------------------------------------------ engine_api

CREATE VIEW engine_api.task_v AS
	SELECT p.task_id, p.project, t.repo, t.workflow_ref, p.title, p.subjects, p.status, p.end_status,
		p.phase, p.node, p."column", p.waiting_on, p.reason, p.text, p.actions,
		-- artifact list without content
		COALESCE((SELECT jsonb_agg(a - 'content') FROM jsonb_array_elements(p.artifacts) a), '[]')
			AS artifacts,
		p.needs_you, p.cost_usd, t.created_at, p.updated_at, t.ended_at
	FROM engine.projection p JOIN engine.tasks t ON t.id = p.task_id;

CREATE FUNCTION engine_api.task_detail(p_task_id bigint) RETURNS jsonb LANGUAGE sql STABLE AS $$
	SELECT to_jsonb(v) - 'artifacts' || jsonb_build_object(
		'artifacts', (SELECT p.artifacts FROM engine.projection p WHERE p.task_id = v.task_id),
		'transitions', COALESCE((SELECT jsonb_agg(jsonb_build_object(
				'at', tr.at, 'event_type', e.type,
				'from', jsonb_build_object('status', tr."from"->>'status', 'node', tr."from"->>'node'),
				'to', jsonb_build_object('status', tr."to"->>'status', 'node', tr."to"->>'node'),
				'effects', jsonb_array_length(tr.effects)) ORDER BY tr.id)
			FROM engine.transitions tr LEFT JOIN engine.events e ON e.id = tr.event_id
			WHERE tr.task_id = v.task_id), '[]'),
		'step_runs', COALESCE((SELECT jsonb_agg(to_jsonb(s) - 'task_id' ORDER BY s.started_at NULLS LAST, s.run_id)
			FROM engine.step_runs s WHERE s.task_id = v.task_id), '[]'))
	FROM engine_api.task_v v WHERE v.task_id = p_task_id
$$;

CREATE FUNCTION engine_api.session_v(p_run_id text, p_after_seq int, p_limit int)
RETURNS TABLE (seq int, at timestamptz, kind text, payload jsonb, truncated boolean)
LANGUAGE sql STABLE AS $$
	SELECT seq, at, kind, payload, truncated FROM engine.session_events
	WHERE run_id = p_run_id AND seq > p_after_seq ORDER BY seq LIMIT p_limit
$$;

CREATE FUNCTION engine_api.session_search(p_project text, p_query text)
RETURNS TABLE (run_id text, task_id bigint, seq int, at timestamptz, kind text, snippet text)
LANGUAGE sql STABLE AS $$
	SELECT e.run_id, s.task_id, e.seq, e.at, e.kind, left(e.payload::text, 200)
	FROM engine.session_events e
	JOIN engine.step_runs s ON s.run_id = e.run_id
	JOIN engine.tasks t ON t.id = s.task_id
	WHERE t.project = p_project AND e.payload::text ILIKE '%' || p_query || '%'
	ORDER BY e.at DESC LIMIT 100
$$;

CREATE FUNCTION engine_api.board_v(p_project text) RETURNS jsonb LANGUAGE sql STABLE AS $$
	SELECT COALESCE((SELECT board FROM engine.boards WHERE project = p_project), '{"columns": []}')
$$;

CREATE FUNCTION engine_api.workflow_v(p_project text, p_repo text) RETURNS jsonb LANGUAGE sql STABLE AS $$
	SELECT view FROM engine.workflow_views WHERE project = p_project AND repo = p_repo
$$;

CREATE FUNCTION engine_api.metrics(p_project text, p_from timestamptz, p_to timestamptz)
RETURNS TABLE (workflow text, phase text, step text, runs bigint, avg_wall_secs double precision,
	active_secs double precision, queue_secs double precision, input_tokens bigint,
	output_tokens bigint, cost_usd numeric, retries bigint, resumes bigint, stalls bigint, steers bigint)
LANGUAGE sql STABLE AS $$
	SELECT t.workflow_ref, split_part(s.node, '/', 1), s.step, count(*),
		avg(extract(epoch FROM s.ended_at - s.started_at))::double precision,
		sum(s.active_secs), sum(s.queue_secs),
		sum((s.tokens->>'input')::bigint), sum((s.tokens->>'output')::bigint), sum(s.cost_usd),
		sum(GREATEST(s.attempt - 1, 0)), sum(s.resumes), sum(s.stalls), sum(s.steers)
	FROM engine.step_runs s JOIN engine.tasks t ON t.id = s.task_id
	WHERE t.project = p_project AND s.started_at >= p_from AND s.started_at < p_to
	GROUP BY 1, 2, 3 ORDER BY 1, 2, 3
$$;

CREATE FUNCTION engine_api.submit(p_task_id bigint, p_actor jsonb, p_kind text, p_data jsonb)
RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE id bigint;
BEGIN
	IF p_kind NOT IN ('command', 'comment', 'cancel', 'resume', 'stop', 'steer') THEN
		RAISE EXCEPTION 'unknown kind %', p_kind USING ERRCODE = '22023';
	END IF;
	INSERT INTO engine.events (dedupe_key, project, task_id, type, payload)
	SELECT 'ui:' || gen_random_uuid(), t.project, t.id, 'ui.' || p_kind,
		jsonb_build_object('source', 'ui', 'actor', p_actor, 'kind', p_kind, 'data', p_data)
	FROM engine.tasks t WHERE t.id = p_task_id
	RETURNING events.id INTO id;
	IF id IS NULL THEN RAISE EXCEPTION 'task % not found', p_task_id USING ERRCODE = 'P0002'; END IF;
	RETURN id;
END $$;

CREATE FUNCTION engine_api.secret_set(p_scope text, p_owner text, p_name text, p_plaintext text)
RETURNS void LANGUAGE sql AS $$
	-- fixture: no encryption
	INSERT INTO engine.secrets (scope, owner, name, ciphertext) VALUES (p_scope, p_owner, p_name, convert_to(p_plaintext, 'UTF8'))
	ON CONFLICT (scope, owner, name) DO UPDATE SET ciphertext = EXCLUDED.ciphertext, updated_at = now()
$$;

CREATE FUNCTION engine_api.secret_list(p_scope text, p_owner text)
RETURNS TABLE (name text, updated_at timestamptz) LANGUAGE sql STABLE AS $$
	SELECT name, updated_at FROM engine.secrets WHERE scope = p_scope AND owner = p_owner ORDER BY name
$$;

-- Not in contracts-v2 yet (proposed): delete one secret; true when it existed.
CREATE FUNCTION engine_api.secret_delete(p_scope text, p_owner text, p_name text)
RETURNS boolean LANGUAGE sql AS $$
	WITH d AS (DELETE FROM engine.secrets WHERE scope = p_scope AND owner = p_owner AND name = p_name RETURNING 1)
	SELECT EXISTS (SELECT 1 FROM d)
$$;

CREATE FUNCTION engine_api.chat_v(p_user text)
RETURNS TABLE (run_id text, project text, title text, started_at timestamptz, last_at timestamptz, events bigint)
LANGUAGE sql STABLE AS $$
	SELECT c.run_id, c.project, c.title, c.started_at,
		(SELECT max(e.at) FROM engine.session_events e WHERE e.run_id = c.run_id),
		(SELECT count(*) FROM engine.session_events e WHERE e.run_id = c.run_id)
	FROM engine.chats c WHERE c."user" = p_user ORDER BY c.started_at DESC
$$;
