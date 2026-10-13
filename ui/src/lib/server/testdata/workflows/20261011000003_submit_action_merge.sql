-- UI actions (approve | cancel | retry | merge) become `ui:` events; returns the event id.
-- Event types: approve -> ApprovalGranted, cancel -> CancelRequested, retry -> RetryRequested,
-- merge -> MergeRequested. `MergeRequested` is engine-internal like `CancelRequested`: only the
-- change_request machine reads it, and it merges only when the PR is ReadyToMerge.
CREATE OR REPLACE FUNCTION workflows_api.submit_action(
    p_work_item_id bigint, p_actor jsonb, p_action text, p_payload jsonb
) RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, workflows, pg_temp
AS $$
DECLARE
    v_key  text;
    v_type text;
BEGIN
    v_type := CASE p_action
        WHEN 'approve' THEN 'ApprovalGranted'
        WHEN 'cancel'  THEN 'CancelRequested'
        WHEN 'retry'   THEN 'RetryRequested'
        WHEN 'merge'   THEN 'MergeRequested'
        ELSE NULL
    END;
    IF v_type IS NULL THEN
        RAISE EXCEPTION 'unknown action %', p_action USING ERRCODE = '22023';
    END IF;

    SELECT key INTO v_key FROM workflows.work_items WHERE id = p_work_item_id;
    IF v_key IS NULL THEN
        RAISE EXCEPTION 'work item % not found', p_work_item_id USING ERRCODE = 'no_data_found';
    END IF;

    RETURN workflows_api.ingest_event(
        'ui',
        'ui:' || gen_random_uuid()::text,
        v_key,
        v_type,
        jsonb_build_object('actor', p_actor, 'action', p_action, 'data', COALESCE(p_payload, '{}'::jsonb))
    );
END;
$$;
