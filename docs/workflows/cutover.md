# Cutover to the ADR-0022 engine on oldie

This is how the YAML engine replaces the old compiled engine on oldie (`enzarb.dev`), where enzarb dogfoods it on enzarb/enzarb#227. Deploys go through the updater, which patches the RKE2 `HelmChart` when a chart is released. Nobody runs `helm` or `mise` against oldie by hand.

## What changes at once

- `workflows` migration 3 drops every object in schemas `workflows`/`workflows_api`, keeping only `workflows._sqlx_migrations` and the schemas themselves (pgop declares them). Old tasks, jobs and transitions are gone. There is no data migration.
  - Checked against a `pg_dump --schema-only` of oldie (2026-10-16), loaded into Postgres 18 with the pgop roles and the `engine`/`engine_api` schemas:
    - migrations 1–4 applied cleanly;
    - `app`, `app_api` and `connectors` kept every object;
    - `enzarb_ui` (task_v, submit), `enzarb_connectors` (ingest), `enzarb_realtime` (task_v), `enzarb_workflows` (tables, `app_api.claude_credential_get`) and `enzarb_sandbox_proxy` (chat functions) have their grants.
- `ui` migration 0008 empties `app.push_item`, whose entries pointed at old tasks.
- Every open issue of every Project repository that has no task gets an `adopted` event when the ticker starts (the adopt pass, idempotent).
  - The library binding `adopted_issues` runs `workflows/issue` for each one.
  - It passes the dependencies phase, then **waits at the start gate**: column backlog, "Start planning". Adoption runs no agent and spends nothing.
  - On oldie today that covers at least #227, #224, #221, #220 and #121 (the old engine's open items), plus any other open issue.
  - Open pull requests are not adopted (`list_open_issues` skips them). A PR gets a task on its next event, through the `pull_requests` binding, or by joining its issue's task.

## Prerequisites (verify each on GitHub before merging)

1. **The library repository** `enzarb/workflows` holds the reviewed library (local branch `redesign`, commit `984bdbb` or later, scenarios green). Tag it `v1` and push the tag: `git tag v1 && git push origin v1`.
   - `ENZARB_DEFAULT_LIBRARY` defaults to `enzarb/workflows@v1`.
   - The Project's repo list on oldie names `enzarb/enzarb-workflows`, not `enzarb/workflows`. Check which one is the real library repo. If the library lives in `enzarb-workflows`, set `workflows.defaultLibrary: enzarb/enzarb-workflows@v1` in the chart values, or the Project's `spec.workflows.library`.
2. **The enzarb GitHub App is installed on the library repository.** The engine reads definitions with the App's installation token. Without it every resolution fails, and events stay unrouted with note `bindings: …`.
3. **The library's dependencies check** must pass on a known-empty `facts.dependencies` (`[]`). Use `facts.dependencies is not none and …` in `phases/dependencies.yaml`, not `facts.dependencies and …`, because `[]` is falsy. Otherwise every task waits for one refresh round before the start gate. That still works, but it is slower.
4. The Project owner `ruckc` has a stored Claude credential: `app.claude_credentials` row present, key `e6f1f335…`, 2026-10-13. Every agent step uses this row as the user-scope secret `claude_token` (`CLAUDE_CODE_OAUTH_TOKEN`). If the row is missing, the job blocks and the task pauses with "missing secret(s) user:claude_token" until it is captured again (Claude tab sign-in) and someone clicks Resume.

## Merge and release order

All of these land in one PR from `redesign/m7`, squash-merged, then released per component in this order:

1. `proto` (additive `step_start.files`).
2. `operator` (CRD `spec.workflows`).
3. `connectors`: catalog with `merge_sha` and `mentioned`. The image now builds from the repo root.
4. `workflows` (engine, migrations 1–4), `sandbox-container` (no step time cap, secret files), `sandbox-proxy` (chat capture), `realtime` (engine_projection stream), `ui` (engine_api, migration 0008).
5. `helm`:
   - engine values;
   - pgop grant `engine_api` USAGE to `enzarb_sandbox_proxy`;
   - the generated `ENZARB_SECRETS_KEY` Secret.

   The updater then rolls oldie to the new chart.

## Values and secrets

- **Nothing new to create by hand.** The chart generates `ENZARB_SECRETS_KEY` once (kept with `helm.sh/resource-policy: keep`) unless `secrets.engineSecretsKey.existingSecret` is set. The Claude keyring Secret `enzarb-claude-key` already exists and is mounted into workflows.
- `workflows.botLogin`/`botHandle` and `connectors.botLogin` default to `enzarb[bot]`/`enzarb`. They must match the App's real bot login. Check it on GitHub: the App slug is `enzarb`.
- `ENZARB_PUBLIC_URL` comes from `publicUrl: https://enzarb.dev`. Task documents use it as `instance.url`.
- Deprecated values are still accepted and ignored: `secrets.executorCallback`, `workflows.npmToken`, `maxAutoRounds`.

## `.enzarb/workflows.yaml` for enzarb/enzarb

Commit this file to enzarb/enzarb (through a normal PR) after `v1` is tagged. It comes from the library's `examples/enzarb.yaml`:

```yaml
version: 1
imports: {std: enzarb/workflows@v1}
bindings:
  - id: issues
    on: issue
    if: "event.action == 'opened'"
    use: std/workflows/issue
    with:
      release_when: [release.published]
      max_rounds: 4
      epic_allowed: "{{ relations | selectattr('kind', 'eq', 'parent') | list | length == 0 }}"
```

Without the file, the library's bindings apply unchanged: issues end at the merge, with 3 fix rounds. The repo layer is read at the default-branch head on every resolution, so tasks pick it up at their next wait.

- Adopted issues use binding `adopted_issues`, so this file does not change their params.
- To give adopted issues `release_when`, add an `adopted_issues` binding with the same `with:`.

`spec.workflows` on the Project is not needed. Set it only to change the library (`library: owner/repo@ref`) or to add Project-wide bindings.

## Starting #227

1. Once the new chart is running, open the board at https://enzarb.dev/projects/enzarb (or the task at https://enzarb.dev/tasks). #227 shows in Backlog as "Waiting to start".
2. Start it in one of two ways: click **Start planning**, or comment `/plan` (or `@enzarb /plan`) on https://github.com/enzarb/enzarb/issues/227 as a writer.
3. Triage runs, then plan. The plan is posted as a comment and an artifact. Approve it with **Approve** or `/approve`. Any other comment is feedback, and the plan is revised.
4. Implement, the checks, the PR, the review (up to 4 fix rounds with the repo file) and CI follow automatically. Then **Merge** (or `/merge`) when it shows "Ready to merge". With `release_when`, the task ends when a component release is published from a tag that contains the merge commit.

Stalls (no session activity for `stallSecs`, default 30 min) pause the task; **Resume** continues the same agent session. Interrupted steps (sandbox restarts or rolls) resume and do not restart.

## Rollback

- **Code:** revert the PR and release the previous component versions. The updater rolls the chart back.
- **Data:** the old engine's tables are gone after migration 3, and that cannot be reverted. A rolled-back old `workflows` image starts on empty `workflows` tables:
  - its own migrations are recorded in `workflows._sqlx_migrations`, which migration 3 keeps;
  - recreating its tables would need that history cleared first: `DELETE FROM workflows._sqlx_migrations` as the owner, then restart.
  - The old tasks are not restored; take a `pg_dump` of `workflows` before the cutover if they matter.
- **Engine only:** to stop the engine without a rollback, scale the workflows Deployment to 0. Webhooks keep landing in `engine.events` (deduplicated) and are processed when it comes back.
