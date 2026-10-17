## Review of #248

**Verdict: comment.** I found no must-fix problems in the diff. I could not get a clean `npm test` run in this sandbox, so I can't approve on checks alone.

### The change
- Renaming the second `.ez-badge` rule to `.ez-nav__count` (`ui/src/lib/styles/global.scss:1437`) and switching to `var(--ez-accent-fg)` is correct. `--ez-accent-contrast` was undefined, so the text always fell back to `#fff`. `--ez-accent-fg` is covered by the existing contrast spec in both themes.
- `Nav.svelte:26` is the only user of the count style. A grep of `ui/src` for `ez-badge` finds only `Badge.svelte`, `StepRuns`, `WorkflowViewer`, `SessionInspector` and `SessionEventView`, which are status chips. Nothing else needs updating.
- The rename also stops the old rule from overriding the status chips with its cyan background and white text. Those chips now use the token-driven rule at `global.scss:812`. This is a visible change for them, and the PR says it wasn't checked in a browser. It is the intended outcome, and a person can confirm it.
- The guard test in `contrast.spec.ts` is simple and targets the root cause.

### Checks
- `npm run check` passed (0 errors, 0 warnings).
- `npm run lint` passed (prettier and eslint).
- `npx vitest run --project server src/lib/styles` passed (64 tests, including the new guard).
- Full `npm test` did **not** pass. It ended with 16 failed tests in 13 files and 1054 passed. The failures I could see were Playwright/chromium timeouts, such as `RemoteForm.svelte.test.ts` hitting the 15s limit. The PR author saw 8 failures in different suites. The differing counts and the timeouts suggest a slow or flaky browser environment rather than this change, since no test touches the Nav or the count class. I did not run the suite on the base commit, so I can't prove that.

### Optional suggestions
- Render the Nav with a count and the Badge gallery in both themes before merge (the PR admits it didn't).
- The `.ez-nav__count` selector is a BEM element of `.ez-nav`. That is fine if `.ez-nav` is the nav's block class, which I did not check.
