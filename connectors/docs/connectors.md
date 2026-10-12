# Connectors: CI status and merge

This covers the CI-status event and the user-triggered merge primitives. enzarb
never merges on its own; workflows and the UI call these when a user clicks
Merge. The broader contract text lives in the top-level `enzarb` repo
(`docs/contracts.md`).

## `CiStatusChanged` event

Wire name `CiStatusChanged`. `data.ci` is additive and optional; payloads
without it still deserialize, and it is omitted from output when absent.

```json
"ci": {
  "head_sha": "9fceb02d...",
  "state": "success | failure | pending",
  "context": "build",
  "url": "https://github.com/o/r/runs/1"
}
```

`context` and `url` are optional. The event carries the result of **one** check
run, check suite or commit status, not the aggregate for the head. The stream
is noisy (one event per completed run and per status), so consumers should treat
an event as a hint to call `get_merge_readiness` rather than fold events.

### GitHub mapping

| Webhook | Handling |
| --- | --- |
| `check_suite` `completed` | One event per entry in `check_suite.pull_requests`. |
| `check_run` `completed` | One event per entry in `check_run.pull_requests`. |
| `status` | `normalize` returns `Normalized::CommitStatus`; `GitHubConnector::resolve_commit_status` looks up open PRs whose head is the commit (`GET /commits/{sha}/pulls`) and builds the events. No PR means no events. |

* Empty `pull_requests` yields no events.
* The first event of a delivery uses the delivery id; further PRs use
  `<delivery>#pr<number>`, so dedupe keys stay distinct.
* `item` is built from the PR stub in the payload: number, `head.sha`,
  `head.ref`, `base.ref` and url are set; title, body and author are empty.
  Fetch the thread if you need them.
* Loop avoidance: deliveries whose `sender` is the bot are dropped, and
  `check_run` / `check_suite` whose `app.slug` is the bot's slug (the login
  without `[bot]`) are dropped too, since the sender can differ from the owning
  App.
* The receiver needs App credentials (`GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY`)
  to resolve `status` deliveries. Without them it ignores `status` (204).

## Aggregation

`aggregate_ci(checks, statuses)` (in `enzarb-connector`) is the single rule, used
by `get_merge_readiness`. It covers **all** checks and statuses on the head, not
only required ones.

* Any failing check or status gives `failure`.
* Otherwise any unfinished check, or a combined status of `pending` that has at
  least one status, gives `pending`.
* Otherwise `success`. A head with no checks and no statuses is `success`
  (GitHub reports `pending` for zero statuses; that is not treated as pending).

Check conclusions: `success`, `neutral`, `skipped` are success; `failure`,
`timed_out`, `cancelled`, `action_required`, `stale` are failure; a missing
conclusion or a run that is not `completed` is pending.

## `Connector::merge_change_request`

```
merge_change_request(item, number, expected_head_sha, commit_title,
                     commit_body, idempotency_key) -> MergeOutcome
```

| Outcome | Meaning |
| --- | --- |
| `Merged { sha }` | This call squash-merged; `sha` is the new commit. |
| `AlreadyMerged { sha }` | The PR was already merged; `sha` is its merge commit when known. The merged head may differ from `expected_head_sha`; compare if it matters. |
| `HeadMoved` | The head is no longer `expected_head_sha`. Nothing merged. |
| `NotMergeable { reason }` | GitHub refused (conflicts, branch rules, ...). `reason` is GitHub's message. |

GitHub: `PUT /repos/{o}/{r}/pulls/{n}/merge` with `merge_method=squash`,
`sha=expected_head_sha`, `commit_title` and `commit_message`. 409 is `HeadMoved`,
405 is `NotMergeable`. The PR is fetched before the PUT and again after a 405 or
409; if it is merged the result is `AlreadyMerged`. Idempotency comes from PR
state: GitHub has no marker for `idempotency_key`, so the GitHub connector
ignores it. Other failures (403, 404, 5xx) are `Err`.

## `Connector::get_merge_readiness`

Returns `MergeReadiness { head_sha, base_sha, mergeable, mergeable_state, ci }`.
`base_sha` is the PR's `base.sha` (no extra API call); it is `None` when absent
and defaults to `None` when deserializing older payloads.

* `mergeable` is `None` while GitHub is still computing it (`null`); it is never
  mapped to `false`. Retry later.
* `mergeable_state` is GitHub's free-form string (`clean`, `dirty`, `blocked`,
  ...), empty when absent.
* `ci` is `aggregate_ci` over `GET /commits/{sha}/status` and
  `GET /commits/{sha}/check-runs` for the PR head (all pages). It costs at
  least three API calls and is not cached.

Workflows uses it to re-evaluate and as the final gate before merging.

## GitHub App configuration

* Repository permissions: Issues RW, Pull requests RW, Contents RW, Checks RW,
  Metadata R, and **Commit statuses: Read** (new). Merging needs Pull requests
  RW and Contents RW, already held.
* Subscribe to events: the existing ones plus **Check suite**, **Check run** and
  **Status** (new).
* Existing installations must accept the new permission before the events and
  status reads work.

## Fake connector

`fake::FakeConnector` records `MergeChangeRequest` / `GetMergeReadiness` calls.
`queue_merge_outcome` scripts the next outcomes (default: `Merged`), a repeated
`idempotency_key` replays the first outcome, and `set_merge_readiness` seeds the
readiness per work item (unseeded is `NotFound`).
