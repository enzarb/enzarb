# Plan: finish the work item → task rename in `ui/` and add a guard (#237)

## Summary
The issue's numbers are stale. Earlier work (#114, #229) already renamed the files, remote functions, components and routes. On `main`, `ui/` still has about 41 case-insensitive matches of `work[ _-]?item`, in about 12 files, and most are comments or SQL names. This PR renames the leftover identifiers and text. It keeps the shared SQL and wire names behind explicit, commented allowlist entries. It also adds a guard script to `npm run lint` that fails if the old name reappears. This is one PR touching `ui/` only.

## Approach
- **Rename mechanically.** Identifiers, comments, UI text and tests become "task".
- **Single data-access layer.** `src/lib/server/push-store.ts` is the only code that calls the `app_api.push_*` functions. Their SQL parameter `work_item_id` (migrations 0006/0007) is a shared contract and stays unchanged. In TS the parameter becomes `taskId`. The SQL is called positionally (`push_claim(${owner}, ${taskId}, …)`), so the rename is safe. A comment at the top of `createPushStore` explains that the SQL still says `work_item` and this file is the only mapping point.
- **Realtime frame topic `'work_item'`** (`live.ts:134`) is a wire contract. Deployed components roll out independently, so it is kept and its existing comment is expanded. It is allowlisted.
- **Routes.** `/work` and `/work/[id]` already 308-redirect to `/tasks` and `/task/[id]`. They are kept, because old push notifications and PR comments link there. The regex needs "work" followed by "item", so the `/work/...` paths never match and need no allowlist entry. The PR description will record the decision: redirects are retained, with no route changes.
- **Guard.** `ui/scripts/check-naming.mjs`, run from `lint` as `prettier --check . && eslint . && node scripts/check-naming.mjs`. It follows the existing `scripts/*.mjs` style.
  - It lists files with `git ls-files -z -- .` run inside `ui/`, which avoids `node_modules` and build output. It falls back to a directory walk if git is unavailable, as in the Docker context.
  - It skips `migrations/` (immutable), `package-lock.json` and binary files.
  - Pattern: `/work[ _-]?item/i`. This also covers `WorkItem`, `workItem`, `work-item`, `work_item` and "work items". A camelCase `workItemId` is caught by the same pattern.
  - The allowlist is a small in-script array of `{ file, pattern, why }` entries. A line is allowed only if it matches that file's pattern. Entries:
    - `src/lib/live.ts` and `src/lib/live.spec.ts`: the `work_item` frame topic.
    - `src/lib/usage.ts`: the comment citing `work_item_v.usage_totals`.
    - `src/lib/server/push-store.ts`: the SQL-mapping comment.
    - The script itself and its spec: the pattern text.
  - It prints `file:line: text` for each violation and exits 1. It also fails if an allowlist entry matches nothing (stale entry), so the list cannot rot.
- **Rejected alternatives:**
  - An ESLint custom rule or `no-restricted-syntax`: it misses comments, markdown and SQL strings.
  - A bare `grep` in package.json: it is not portable, and it can't express per-file allowlists.
  - Renaming SQL names: forbidden by the issue and by the shared-contract rule in CLAUDE.md.

## Changes
- `ui/scripts/check-naming.mjs` (new): the guard described above. It exports the check function so a spec can import it.
- `ui/scripts/check-naming.spec.ts` (new): see Tests. Check that vitest's `include` picks up `scripts/`; otherwise place the spec under `src/lib/`.
- `ui/package.json`: add the guard to the `lint` script.
- `ui/src/lib/server/push-store.ts`: `workItemId` → `taskId` (4 places, lines ~96–134). Doc comment "work item" → "task". Add the SQL-mapping comment.
- `ui/src/lib/live.ts` (~134): expand the comment saying `work_item` is the legacy frame name kept for the realtime contract.
- `ui/src/lib/usage.ts:3`: keep the SQL view name and add a note that it is a SQL name. Allowlisted.
- `ui/src/lib/components/ui/StepList.svelte:30`: comment "work item's history" → "task's history".
- `ui/src/lib/server/project-onboarding.ts:206,282`: "work items" → "tasks", in the comment and the log message. Update any test that asserts on the log text.
- `ui/src/lib/server/project-onboarding.spec.ts:459`: the `describe` title.
- `ui/src/lib/sw-handlers.spec.ts:104`: `getWorkItem` → `getTask`. That remote function exists in `tasks.remote.ts`, and `getWorkItem` does not. The `/work/...` URLs in this spec and in `push.spec.ts` stay as legacy-URL fixtures.
- `ui/README.md:37`: reword the "work item" mention.
- Not edited: `migrations/0006–0008` (excluded by the guard).

## Tests
- Guard spec: use a temp directory fixture.
  - A clean tree passes.
  - A file containing `WorkItem`, `work-item`, `work item` or `workItemId` fails.
  - An allowlisted line passes.
  - A stale allowlist entry fails.
  - Migrations are ignored.
- Manual acceptance check: add `// WorkItem` to a `src/` file, confirm `npm run lint` fails with `file:line`, then revert.
- Existing specs cover `push-store`, `live`, `sw-handlers` and `project-onboarding` after the renames. Check that `work/[id]/page.server.spec.ts` still passes.
- Run in `ui/`: `npm run check && npm run lint && npm test`.

## Risks and open questions
- **`app_api` calls.** The only risk is a typo in the `push-store.ts` rename. The existing push-store tests, plus `npm run check`, should catch it.
- **Realtime.** The `work_item` topic must keep being handled. The `live.spec.ts` test stays as is.
- **Assumption:** keeping the `/work` redirects permanently is acceptable. Say so if you want them removed after some deprecation window.
- **Assumption:** `git ls-files` is available in CI. `actions/checkout` provides it, and the fallback walk covers Docker.
- **Wording to confirm:** whether the "tasks" in `project-onboarding` messages should say "tasks" or "workflow tasks". I plan plain "tasks".

## Out of scope
- Renaming SQL objects, columns or migrations in `workflows_api` / `app_api`.
- Changing the realtime frame topic.
- `docs/`, `connectors/` and `workflows/` occurrences of the term, which belong to the shared contract.
- Any change to the `/work` redirects.

Reply with feedback to revise this plan, or comment /approve to start the implementation.
