## Summary

In `ui/`, replace the two GitHub App token minters with one `token(repo, perms)` singleton. It has one JWT signer and one cache. Add one `githubFetch` helper that owns `https://api.github.com`, and use it in `auth.ts`, `github-repos.ts`, `releases.ts` and `user-repositories.ts`. Delete `github-app-token.ts` and its spec, and move its coverage into `github-app.spec.ts` and `github-repos.spec.ts`.

## Approach

- **Where `githubFetch` goes.** It goes in `github-app.ts`, next to `githubHeaders` and `signAppJwt`. That file already owns `GITHUB_API`, which becomes private, so no new module is needed. Signature: `githubFetch(path, init & { token?: string; fetch?: typeof fetch }): Promise<Response>`.
  - **Request and headers.** It builds `API + path` and sets the standard headers from `githubHeaders`, with `Authorization` only when a token is given. `init.headers` can override them. It passes `redirect`, `signal` and the rest of `init` through.
  - **Callers keep their own policy.** Timeouts, `redirect: 'error'` and error mapping stay with the caller, so nothing changes silently.
  - **Absolute URLs.** It accepts either a path starting with `/` or an absolute URL that starts with `${API}/`. Anything else throws. This keeps the "never send a token anywhere but the GitHub API" guard from `user-repositories.ts`, and `nextLink` pagination URLs are absolute.
- **Singleton.**
  - `createRepoTokenSource` stays as the injectable factory, so tests keep working.
  - `github-app.ts` gains a lazy module-level instance built from `getConfig()` (`githubAppId`, `githubAppPrivateKey`), with exports `token(repo, perms)` and `invalidateToken(repo, perms)`.
  - Its cache and permission semantics are exactly today's `createRepoTokenSource`: per-repo installation id with a 1h TTL, tokens keyed by repo (lowercased) and sorted permissions, a 5 min refresh margin, a shared in-flight mint, `null` on any failure, no caching of failures, and `redirect: 'error'` with a 10s timeout.
  - The factory's internal calls move onto `githubFetch`, so there is a single JWT signer (`signAppJwt`) in the minter.
- **Releases token.**
  - `getInstallationToken` becomes `token(RELEASES_REPO, { contents: 'read' })`. `invalidateInstallationToken` becomes `invalidateToken(RELEASES_REPO, { contents: 'read' })`. The `about` route calls these directly, or through two tiny helpers in `releases.ts`.
  - **Import cycle.** `github-app-token.ts` currently imports `RELEASES_REPO` from `releases.ts`. After the change `releases.ts` imports `githubFetch` from `github-app.ts`. If `github-app.ts` does not import `releases.ts`, there is no cycle, because the `about` route is the only place that combines them.
- **`user-repositories.ts`.**
  - It has no literal URL, but it imports `GITHUB_API` and builds its own `${API}/…` URLs. It also has its own JWT call and a per-installation-id token cache (`metadata: read`, not repo-scoped).
  - That token is a different kind: an installation-scoped token with no `repositories` filter, used to list everything an installation can see. `token(repo, perms)` cannot express it.
  - Scope here is therefore limited. It switches to `githubFetch` and drops its local `API` const and URL guard (now inside the helper). It keeps `signAppJwt` and its installation-token cache, with the same behaviour. It does not use `token()`.
  - **Rejected alternative:** a second `installationToken(id, perms)` method on the minter. That would be new API surface beyond the issue and would change that file's cache semantics.
- **`releases.ts`.** `request()` calls `githubFetch(`/repos/${RELEASES_REPO}/releases?per_page=100`, { token: bearer, fetch: doFetch, signal })`. Its headers are unchanged except for the `X-GitHub-Api-Version` header and `Content-Type`, which the shared headers add. The 401/403 retry stays as is.
- **`auth.ts`.** `githubFetch('/user', { token: accessToken })`. The `GET` call now also sends the shared headers, which is harmless.
- **`github-repos.ts`.**
  - Delete the local `API` const and `headers()`. `send()` and `installationCoversAll` call `githubFetch` with `token`, `fetch`, `redirect: 'error'` and a timeout.
  - `Content-Type: application/json` is now sent on GETs too (the existing minter already does this).
  - Error-to-`RepoCreateError` mapping is unchanged.

## Behaviour differences (all on the releases token only)

| Aspect | Before | After |
|---|---|---|
| Mint timeout | 5s | 10s |
| Redirects | followed | `redirect: 'error'` |
| Installation id | number or string | number only |
| Cache | single entry | per-repo cache, same 5 min margin |

Failures still resolve `null`, and the about page falls back to unauthenticated fetches as it does today.

## Changes

- `ui/src/lib/server/github-app.ts`:
  - add `githubFetch`;
  - make `GITHUB_API` private;
  - route the minter through `githubFetch`;
  - add the lazy singleton with `token` and `invalidateToken`;
  - add a test-only reset hook for the singleton, e.g. `resetTokenSingleton()`.
- `ui/src/lib/server/github-app-token.ts` and `github-app-token.spec.ts`: delete.
- `ui/src/lib/server/auth.ts`, `github-repos.ts`, `releases.ts`, `user-repositories.ts`: use `githubFetch`, as above.
- `ui/src/routes/(app)/about/+page.server.ts` and `page.server.spec.ts`: switch the import and mock to `github-app.ts` (`token` and `invalidateToken`).

## Tests

- **`github-app.spec.ts`** gains the cases it lacks from the deleted spec:
  - the minted token is down-scoped to the repository with `contents: read`, and the JWT is RS256 with a 60s backdate;
  - refresh happens after `expires_at - margin`;
  - `invalidate` forces a re-mint;
  - the in-flight mint is shared;
  - 404 and mint errors give `null` and are not cached;
  - no credentials means no GitHub call;
  - a bad key or network error gives `null` with no key material in the result;
  - the singleton reads config lazily and reuses one cache;
  - `githubFetch` has its own cases: base URL and headers, optional auth, rejection of a foreign absolute URL.
- **`github-repos.spec.ts`:** `githubFetch` is covered via the existing `createRepository` / `installationCoversAll` cases. Add a case that a mocked `fetch` receives `https://api.github.com…` URLs with the Bearer header and `redirect: 'error'`.
- **`releases.spec.ts`, `auth.spec.ts`, `user-repositories.spec.ts`, about `page.server.spec.ts`:** keep passing, and adjust only the headers or mocks they assert on.
- **Checks:** in `ui/`, run `npm run check && npm run lint && npm test`. Then run `git grep -n "api.github.com" ui/src`. It should show only the helper, plus spec mock URLs, which may stay.

## Risks and open questions

- "Cache and permission semantics identical to today" is read as the repo-token source's behaviour. That is the source of the releases-token differences listed above. If a maintainer wants the releases mint to keep its 5s timeout, `createRepoTokenSource` can take a `timeoutMs` option.
- The `user-repositories.ts` scoping above is an interpretation of "use it in the four files". The issue says `token(repo, perms)` is the only minter, but that file's unscoped installation tokens do not fit the signature. Please confirm leaving them there is acceptable.
- The singleton holds process-global state, so specs must reset it between cases. The test-only reset hook covers this.
- No cross-component contracts or Helm values are touched. This should be one PR, typed `refactor(ui): …` or `fix(ui): …`.

## Out of scope

- Moving `user-repositories.ts` installation tokens onto the new minter.
- Changing token TTLs, permissions or the `RepoPermissions` type.
- Any change to OAuth flows, retry policy or release-fetch caching.
- Other components.

Reply with feedback to revise this plan, or comment /approve to start the implementation.
