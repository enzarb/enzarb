# Plan: live repository list (#227)

## Summary
Replace the sign-in snapshot (`app.user_repositories`) with a live list: the repository picker reads from GitHub when it opens or when **Refresh** is clicked, and write access is re-checked live when a repository is added. This is option 1 from the issue. No user token is stored.

Commit `a4a5949` on `origin/fix/live-repo-list` already implements most of this. I could only read it from the local object store (`git fetch` failed here with no GitHub credentials), so it may be slightly behind the remote. I have not run it. It is not on `main`, and I can't tell whether a PR exists for it. The plan is to rebase that commit onto current `main`, close the gaps below, and verify. I would not rewrite it.

## Approach
- **Live list.** The branch reads the repositories every App installation can access (installation token from `github-app.ts`). It keeps the ones where the user has `write`, `maintain` or `admin`. This is done in a new `lib/server/repo-access.ts` plus a rewritten `lib/server/user-repositories.ts`.
- **Display cache.** The list is cached per user for about 60 s, for display only. Refresh bypasses the cache. Authorization never reads it. This keeps the permission-check cost (one GitHub call per repository) down while the picker is open.
- **Live check at add time.** `createProjects`, `setRepositories` and `startCreateRepo` call `getRepositoryAccess().check(login, key)` instead of reading a snapshot. Revoked access is refused immediately. There is no age limit any more.
- **Snapshot table.** Migration `0008_user_repositories_deprecated.sql` marks it deprecated. It is not dropped, because the old UI may still be running during a rolling deploy (`app_api`/`app` changes must be additive per CLAUDE.md). The table is no longer read or written. A follow-up migration drops it later.
- **Rejected alternatives.** Option 2 (store the refresh token) means more secret handling, plus expiry and revocation. Option 3 (OAuth round-trip on Refresh) adds a redirect on every refresh.

## Changes
1. Rebase `a4a5949` onto `main`. Expect conflicts in the `ui/` files it touches: `+layout.svelte`, `settings/+page.svelte`, `projects/[name]/settings/+page.svelte`, and the fake-project-remote fixture. `main` has moved on with the workflow engine rewrite (#229) and dependency bumps. Check the migration number is still free after the rebase.
2. **Gap: "App isn't installed on a repository".** The branch only lists repositories the App can reach, so a repository the user can write to but the App can't see is simply absent. The picker needs to say so.
   - **Mechanism (to confirm with the maintainer).** GitHub gives no direct way to list "repos I can write to that lack the App". The approach below only finds an unreachable repository once the user names it, so it covers repositories created through enzarb and a typed or pasted `owner/name`:
     - when a repository is absent or `check()` returns "no installation", show "enzarb GitHub App is not installed on `<owner>/<repo>`" with a link to the installation settings;
     - for an org: `https://github.com/organizations/<org>/settings/installations`;
     - for a user: `https://github.com/settings/installations`;
     - or the App's install URL, which `InstallBanner` already uses.
   - Add a distinct result code (for example `app-not-installed`) in `repo-access.ts` and surface it in `RepositoriesForm.svelte` and `NewProjectForm.svelte`.
   - Check what `GitHubInstallations.svelte` already shows and reuse it.
3. Pagination and multiple installations in the list call. Make sure `GET /installation/repositories` follows paging (`per_page=100`, all pages), covers every installation, and runs the permission checks with bounded concurrency.
4. Failures. A GitHub failure must show "could not read your repositories, use Refresh" (already in `projects.remote.ts`) and must never fall back to a stale allow-list. If the App is not configured, `createProjects` returns `unavailable`.
5. Leftover references. `grep` for `user_repositories`, `syncUserRepositories` and `getUserRepositorySnapshot`. Nothing should read or write the table, and the login callback should keep not storing the user token.
6. Docs: `docs/contracts.md` and `docs/oldie.md` (already edited on the branch). Re-read them after the rebase. Mention the 60 s display cache and the live check.

## Tests
- **Branch specs.** Rebase and fix these: `user-repositories.spec.ts`, `project-onboarding.spec.ts`, `projects.remote.spec.ts`, `create-repo-intent.spec.ts`, `callback.spec.ts`, `settings/page.server.spec.ts`, `RefreshRepositoriesButton`, `InstallBanner`, `GitHubInstallations`, `NewProjectForm`, `RepositoriesForm`, `migrate.spec.ts`, `auth.spec.ts`.
- **New.**
  - Paged `/installation/repositories` across several installations.
  - Write, maintain and admin are kept; read and triage are dropped.
  - A failed permission check for one repository does not hide the rest.
  - The cache is bypassed by Refresh.
  - Revoked access is refused at add time even while the cache is warm.
  - The "App not installed on `<repo>`" message and link, for both a user and an org owner.
- **Checks.** In `ui/`: `npm run check && npm run lint && npm test`. Also run the Playwright e2e suite (stubs in `ui/e2e/stubs/repo-access.ts`) if the environment allows. The branch changed `ui/e2e/serve.ts`, `ui/e2e/constants.ts` and `ui/vite.e2e.config.ts`, so check they still work after the rebase.

## Risks and open questions
- **Rate limits.** One permission call per repository on each uncached list. The 60 s cache and bounded concurrency reduce this. An account with hundreds of repositories may still be slow; consider a cap or progressive loading if it shows up.
- **Not-installed message.** As described in item 2, the message can only appear for a repository the user names or just created. Is that enough for the "Done when" item?
- **Cache is per process.** Another UI replica won't see a Refresh on this one. That is acceptable for a display-only cache.
- **Rolling deploy.** The old UI still reads the snapshot, so the table stays until a later release.
- **Assumptions.** Option 1; table deprecated and not dropped; a short display cache is acceptable.

## Out of scope
- Dropping `app.user_repositories` (separate follow-up release).
- Storing user tokens (options 2 and 3).
- Any change to `workflows_api` or other components.

Reply with feedback to revise this plan, or comment /approve to start the implementation.
