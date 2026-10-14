-- Adds the `comment` action (CommentAdded): the UI sends feedback on an issue's plan. Additive;
-- the other actions are unchanged. `data.body` is the comment text. The engine posts it to the
-- issue and treats it like a human comment (a `CommentAdded` with source `ui`; connector events
-- of the same type are told apart by the `action` key). `merge` additionally reads the optional
-- `data.delete_branch` flag, which needs no change here.
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
        WHEN 'approve'      THEN 'ApprovalGranted'
        WHEN 'cancel'       THEN 'CancelRequested'
        WHEN 'retry'        THEN 'RetryRequested'
        WHEN 'merge'        THEN 'MergeRequested'
        WHEN 'repo-removed' THEN 'RepoRemoved'
        WHEN 'comment'      THEN 'CommentAdded'
        ELSE NULL
    END;
    IF v_type IS NULL THEN
        RAISE EXCEPTION 'unknown action %', p_action USING ERRCODE = '22023';
    END IF;
    IF p_action = 'comment' AND (
        jsonb_typeof(p_payload->'body') IS DISTINCT FROM 'string'
        OR btrim(p_payload->>'body') = ''
        OR char_length(p_payload->>'body') > 20000
    ) THEN
        RAISE EXCEPTION 'comment needs a non-empty data.body of at most 20000 characters'
            USING ERRCODE = '22023';
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
