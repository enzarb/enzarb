-- Every webhook delivery is logged here and nothing else happens to it.
-- The installations table was only read by the removed workflow engine.
DROP TABLE IF EXISTS connectors.installations;

CREATE TABLE connectors.events (
    id          bigserial PRIMARY KEY,
    source      text        NOT NULL DEFAULT 'github',
    delivery_id text        NOT NULL,
    event       text        NOT NULL,
    action      text,
    -- Lowercase `owner/repo`; NULL for deliveries that name no repository (ping, installation).
    repo        text,
    sender      text,
    payload     jsonb       NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (source, delivery_id)
);
CREATE INDEX events_repo_id_idx ON connectors.events (repo, id DESC);

-- The only surface other components read.
CREATE VIEW connectors.event_v AS
    SELECT id, source, delivery_id, event, action, repo, sender, payload, received_at
    FROM connectors.events;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_connectors') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA connectors TO enzarb_connectors;
        GRANT USAGE ON ALL SEQUENCES IN SCHEMA connectors TO enzarb_connectors;
        ALTER DEFAULT PRIVILEGES IN SCHEMA connectors GRANT USAGE ON SEQUENCES TO enzarb_connectors;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_ui') THEN
        GRANT USAGE ON SCHEMA connectors TO enzarb_ui;
        GRANT SELECT ON connectors.event_v TO enzarb_ui;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_realtime') THEN
        GRANT USAGE ON SCHEMA connectors TO enzarb_realtime;
        GRANT SELECT ON connectors.event_v TO enzarb_realtime;
    END IF;
END
$$;
