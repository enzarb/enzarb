## Summary

In dark mode the cyan count pill in the nav (the "items need you" badge on the Work link) has white text on `#00f0ff`, about 1.3:1. I found the cause: `ui/src/lib/styles/global.scss` has a second, older `.ez-badge` rule (~line 1437, "Count of items waiting on the user"). It overrides the status-chip `.ez-badge` rule (~line 812) with `background: var(--ez-accent)` and `color: var(--ez-accent-contrast, #fff)`. `--ez-accent-contrast` is not defined in any theme, so the colour always falls back to `#fff`. That is fine on the light theme's dark-cyan accent, but unreadable on the dark theme's bright cyan. The theme already defines the correct token, `--ez-accent-fg` (`#0b0f17` in dark, `#fff` in light). The fix is to use it.

The same rule also leaks into every other `.ez-badge` (Badge.svelte, tasks views), because the two rules share a class. Those chips get the cyan background and `#fff` text too, wherever the status rule doesn't set `background` and `color` itself. I haven't rendered the page to confirm this, so the implementation should check it.

## Approach

- Give the nav count pill its own class (`ez-nav__count`) instead of reusing `.ez-badge`. Its styles use the `--ez-accent` / `--ez-accent-fg` token pair, which already passes the contrast checks in both themes.
- Rejected: only changing `--ez-accent-contrast, #fff` to `--ez-accent-fg`. This fixes the contrast, but the colliding class would still override the status badges and could change their appearance. Splitting the class is the cleaner fix.
- Rejected: defining a new `--ez-accent-contrast` token. It would duplicate `--ez-accent-fg`.
- The change stays token-based, in the single `global.scss`, with no new CSS. Both the `[data-theme='dark']` and `prefers-color-scheme` paths use the `theme-dark` mixin, so the token fix covers both.

## Changes

- `ui/src/lib/styles/global.scss`
  - Rename the ~line 1437 rule from `.ez-badge` to `.ez-nav__count`. Change its colour to `var(--ez-accent-fg)` and drop the `--ez-accent-contrast` fallback. Keep the layout properties.
  - Check that the status `.ez-badge` rule (~line 812) now applies cleanly to `Badge.svelte`, `StepRuns`, `WorkflowViewer`, `SessionInspector` and `SessionEventView`.
- `ui/src/lib/components/Nav.svelte`: change `class="ez-badge"` to `class="ez-nav__count"` on the count span.
- Grep for other users of the nav-count style (for example tests or other components that query `.ez-badge` for the count) and update them.

## Tests

- `ui/src/lib/styles/contrast.spec.ts` already asserts `accent-fg` on `accent` ≥ 4.5:1 for both themes.
- Add a guard test that fails on the root cause, alongside the existing assertions: `global.scss` must not reference `--ez-accent-contrast`. Use a regex over the file, or assert that every `var(--ez-…)` name used is defined in a theme mixin.
- If `Nav.svelte` has a component test, update it for the new class.
- Run in `ui/`: `npm run check && npm run lint && npm test`.
- Manually render the Nav with a count and the Badge gallery in both `data-theme='dark'` and the OS dark preference. I haven't done this yet.

## Risks and open questions

- The issue has no description. I'm assuming "cyan pills with white text" means the nav count pill (the only white-on-cyan combination I found). If the maintainer means something else, such as the accent-tone `Badge`, please say so. The accent tone in dark uses `#00f0ff` text on `#0a2833`, which looks fine.
- The stray `.ez-badge` override may have been changing other badges. Visually check them after the split, in both themes.
- The new nav-count styles use bright cyan with dark text in dark mode. That is the same pairing as the primary button, so it should look consistent.

## Out of scope

- A broader audit or redesign of badge tones or the colour palette.
- Any changes outside `ui/`.

Reply with feedback to revise this plan, or comment /approve to start the implementation.
