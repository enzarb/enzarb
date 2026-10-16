# Connectors: CI status and merge

This covers the CI-status event and the user-triggered merge primitives. enzarb
never merges on its own; workflows and the UI call these when a user clicks
Merge. The broader contract text lives in the top-level `enzarb` repo
(`docs/contracts.md`).

## Inbound: the event catalog (ADR-0022)

The receiver maps each verified GitHub delivery with
`enzarb_connector_github::catalog::map` to `enzarb_flow::events::Event`s (the
catalog in `workflows/crates/flow/src/events.rs`) and calls, per event, in one
transaction:

```sql
SELECT engine_api.ingest($1 dedupe_key, $2 project, $3 subject_key, $4 type, $5::jsonb payload)
-- bigint, NULL for a duplicate dedupe key
```

* `dedupe_key` = `github:<delivery>:<i>` (`i` = index within the delivery),
  `project` = NULL (the engine routes by repo), `subject_key` =
  `github:<owner>/<repo>#<n>` (lowercase) when the event is about an issue or
  pull request, else NULL, `type` = `Event::type_name()`, `payload` = the
  serialized event.
* Envelope: `id` = delivery id, `repo` = `owner/repo` lowercase, `actor` =
  `{login, can_write: false, bot}` (the engine fills `can_write`), `subject`,
  `at` (payload time, else the mapping time).

| Webhook | Catalog `type` |
| --- | --- |
| `issues` opened/edited/closed/reopened | `issue` |
| `issues` / `pull_request` labeled/unlabeled | `label` (`added`/`removed`, name lowercased) |
| `issue_comment` created, `pull_request_review_comment` created | `comment` (`command`/`args` parsed, leading `@<handle>` allowed) |
| `push` to a branch (not deletes, not tags) | `push` |
| `create` / `delete` with `ref_type: tag` | `tag` (`created`/`deleted`) |
| `pull_request` opened/closed/synchronize/edited/reopened/ready_for_review | `pull_request` (`references` from closing keywords in the body) |
| `check_run` (any action) | `check_run`, one per PR in `pull_requests` (or one without a subject) |
| `check_suite` completed, `status` | `ci_status` |
| `pull_request_review` submitted | `review` |
| `release` | `release` |
| `registry_package` / `package` published | `package` |
| `deployment_status` | `deployment` |
| `workflow_run` | `workflow_run`, one per PR |

Loop protection as before: comments, reviews, issue and label events sent by
the bot login are dropped (except an epic sub-issue opening), and check runs and
suites owned by the bot's App. `installation` / `installation_repositories`
still update `connectors.installations`. Golden tests: `crates/connector-github/tests/catalog.rs`.

### Custom events

`POST /v1/events/<project>` with `X-Enzarb-Signature: sha256=<hex HMAC-SHA256 of
the body>` and body `{name, payload, id?, subject?: {kind, number}, repo?}`
ingests a `custom` event (`project` = `<project>`, `dedupe_key` =
`custom:<project>:<id or sha256(body)>`). The HMAC key is the project secret
named by `ENZARB_EVENTS_SECRET_NAME` (default `events_hmac`), read with
`SELECT ciphertext, pending FROM engine_api.secret_for_receiver($1 project, $2 name)`:
`pending` is plaintext; otherwise `ciphertext` = 12-byte nonce || AES-256-GCM
ciphertext+tag under `ENZARB_SECRETS_KEY` (base64 of 32 bytes). 401 missing/bad
signature, 404 no secret, 400 malformed body, 202 accepted. The engine validates
the payload against the project's declared schema.

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

## `Connector::set_base`

```
set_base(item, number, new_base, idempotency_key) -> ()
```

Retargets a change request onto `new_base` (stacked change requests). Setting the
base it already has succeeds. The default implementation returns `Err`.

GitHub: `PATCH /repos/{o}/{r}/pulls/{n}` with `{"base": new_base}`. GitHub answers
200 when the base is unchanged, so the call is naturally idempotent and
`idempotency_key` is ignored. A 422 (unknown base branch) is `Error::Invalid`, 404
is `Error::NotFound`. The fake updates `base_ref` on the seeded thread and on opened
change requests, and records calls (`FakeConnector::set_base_calls()`).

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
