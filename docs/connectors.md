# Connectors (`enzarb-connectors`)

A connector links enzarb to an outside system that can start or receive work. GitHub is the first, but nothing outside this repo depends on GitHub ([ADR-0007](adr/0007-connectors.md)).

Planned connectors: GitHub (first), then GitLab, Forgejo/Gitea, Bitbucket, Jira/Linear, Slack.

The name `connectors` was chosen to avoid terms that already mean something in Kubernetes (ingress, gateway, router, proxy, controller, scheduler).

## Repo layout
- `crates/connector`: the `Connector` trait, normalized event types, shared helpers (signature checking, retry policy).
- `crates/connector-github`: GitHub App implementation.
- `bin/receiver`: HTTP service with one route per connector (`/hooks/github`, …).

The `enzarb-workflows` executor uses the library crates to make outbound calls.

## Inbound: normalized events

The receiver checks the provider's signature and **normalizes** the payload. It then calls `workflows_api.ingest_event(source, dedupe_key, work_item_key, type, payload)` and returns 200, all within GitHub's 10s budget. It does no workflow logic and makes no outbound calls.

| Event | Meaning | GitHub source |
|---|---|---|
| `WorkItemOpened` | New issue / ticket | `issues.opened` |
| `CommentAdded` | Comment on an item or change request | `issue_comment.created`, `pull_request_review_comment.created` |
| `Mentioned` | `@enzarb …` addressed to the bot | parsed from comment bodies |
| `ApprovalGranted` | Plan approved | `/approve` comment, or the `enzarb:approved` label (`issues.labeled`) |
| `ChangeRequestOpened` | PR / MR opened or reopened | `pull_request.opened`, `reopened` |
| `ChangeRequestUpdated` | New commits pushed | `pull_request.synchronize` |
| `ReviewSubmitted` | Review on a change request | `pull_request_review.submitted` |
| `WorkItemClosed` | Closed or merged | `issues.closed`, `pull_request.closed` (`merged` flag) |

The common envelope uses CloudEvents attributes:

```json
{
  "source": "github",
  "id": "<delivery id>",
  "type": "CommentAdded",
  "subject": "github:org/repo#123",
  "time": "…",
  "data": { "actor": {"source": "github", "login": "…", "id": 42},
            "item": {"kind": "issue", "number": 123, "url": "…"},
            "comment": {"id": 987, "body": "…"} }
}
```

**Approval detection belongs to the connector.** GitHub sends no webhook for reactions, so on GitHub approval is a `/approve` comment or a label. GitLab does send emoji webhooks, so its connector can map 👍 on the plan comment to `ApprovalGranted`. Workflows only ever see `ApprovalGranted`.

## Outbound: the `Connector` trait

```rust
#[async_trait]
pub trait Connector {
    async fn fetch_thread(&self, item: &ItemRef) -> Result<Thread>;
    async fn post_comment(&self, item: &ItemRef, body: &str) -> Result<CommentRef>;
    async fn open_change_request(&self, req: ChangeRequestSpec) -> Result<ItemRef>;
    async fn set_status(&self, target: &CommitRef, status: Status) -> Result<()>;
    async fn set_progress(&self, target: &ProgressTarget, p: Progress) -> Result<()>;
    async fn check_permission(&self, repo: &RepoRef, actor: &Actor) -> Result<Permission>;
    async fn mint_clone_credential(&self, repo: &RepoRef, scope: CredScope) -> Result<ShortLivedCred>;
}
```

- `mint_clone_credential` returns a **short-lived, single-repo** token: a GitHub App installation token restricted to that one repository with minimal permissions. It is handed to sandbox-init for each run, and it is never the App's private key.
- Every call accepts an idempotency key, so repeated job executions don't double-post (for example, a hidden marker in the comment body checked before posting).

## Progress

`set_progress(target, Received | Working | Done | Failed)` lets people know their request was picked up.

| Progress | GitHub reaction |
|---|---|
| Received | 👀 `eyes` |
| Working | 🚀 `rocket` |
| Done | 👍 `+1` |
| Failed | 😕 `confused` |

- The connector removes the previous reaction **it** added, so only the current state shows.
- These calls are queued by the **reducer** once it accepts an event, never by the receiver. The receiver doesn't yet know whether the event is a duplicate, unauthorized or irrelevant.
- They are high-priority jobs that skip agent concurrency caps.
- Connectors without reactions fall back to a label, status or short comment.

## Connector state
Schema `connectors` (see [database.md](database.md)) holds:
- GitHub App installations: installation id ↔ account ↔ repos;
- connector accounts and webhook secrets, by reference to k8s Secrets and never stored inline;
- polling cursors, for sources without webhooks;
- cached installation tokens, encrypted and with expiry.

## Testing
- Recorded webhook payload fixtures per event type. Normalization gets golden tests.
- A trait conformance suite each connector must pass, run against a fake provider.
- GitHub can't reach a local kind cluster, so live testing goes through a smee-style relay.
