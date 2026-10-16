-- ADR-0022: the old engine's objects go, with no data migration. The schemas themselves stay
-- (pgop declares them); only their contents are dropped.
DO $$
DECLARE
    r record;
BEGIN
    FOR r IN SELECT n.nspname, c.relname, c.relkind FROM pg_class c
             JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname IN ('workflows_api', 'workflows') AND c.relkind IN ('v', 'm')
    LOOP
        EXECUTE format('DROP %s IF EXISTS %I.%I CASCADE',
                       CASE r.relkind WHEN 'v' THEN 'VIEW' ELSE 'MATERIALIZED VIEW' END, r.nspname, r.relname);
    END LOOP;
    FOR r IN SELECT n.nspname, p.oid::regprocedure AS sig FROM pg_proc p
             JOIN pg_namespace n ON n.oid = p.pronamespace
             WHERE n.nspname IN ('workflows_api', 'workflows')
    LOOP
        EXECUTE format('DROP ROUTINE IF EXISTS %s CASCADE', r.sig);
    END LOOP;
    FOR r IN SELECT n.nspname, c.relname FROM pg_class c
             JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'workflows' AND c.relkind IN ('r', 'p') AND c.relname <> '_sqlx_migrations'
    LOOP
        EXECUTE format('DROP TABLE IF EXISTS %I.%I CASCADE', r.nspname, r.relname);
    END LOOP;
    FOR r IN SELECT n.nspname, t.typname FROM pg_type t
             JOIN pg_namespace n ON n.oid = t.typnamespace
             WHERE n.nspname IN ('workflows_api', 'workflows') AND t.typtype IN ('e', 'c', 'd')
               AND NOT EXISTS (SELECT 1 FROM pg_class c WHERE c.reltype = t.oid)
    LOOP
        EXECUTE format('DROP TYPE IF EXISTS %I.%I CASCADE', r.nspname, r.typname);
    END LOOP;
END
$$;
