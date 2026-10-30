-- tracks (ADR-0029) reads the event log through the view, like ui and realtime. Additive.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_tracks') THEN
        GRANT USAGE ON SCHEMA connectors TO enzarb_tracks;
        GRANT SELECT ON connectors.event_v TO enzarb_tracks;
    END IF;
END
$$;
