-- A change request run by a graph workflow (`graph:change_request`, ADR-0020) inherits the
-- dependencies of the issue its body closes, like a compiled `change_request`. Same columns as
-- before, so the views built on it are unchanged.
CREATE OR REPLACE VIEW workflows.effective_dependencies AS
    SELECT work_item_key, depends_on_key, source, NULL::text AS via
    FROM workflows.work_item_dependencies
    UNION
    SELECT w.key, d.depends_on_key, d.source, d.work_item_key
    FROM workflows.work_items w
    CROSS JOIN LATERAL regexp_matches(
        coalesce(w.state_data->>'body', ''),
        '(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#([0-9]+)', 'gi') AS m(g)
    JOIN workflows.work_item_dependencies d
      ON d.work_item_key = regexp_replace(w.key, '#[0-9]+$', '') || '#' || m.g[1]
    WHERE w.workflow IN ('change_request', 'graph:change_request') AND d.depends_on_key <> w.key;
