# 0010. Plan approval and progress signals

## Context
People approve agent plans, and they want to see that a request was picked up. GitHub sends no webhook for reactions.

## Decision
- **Approval on GitHub:** a `/approve` comment or the `enzarb:approved` label, from a user with write access. Both arrive as webhooks immediately. Other connectors decide how they detect approval (GitLab can use emoji events), but all of them emit `ApprovalGranted`.
- **Progress:** emoji reactions on the triggering comment or item, via `Connector::set_progress`. 👀 received → 🚀 working → 👍 done / 😕 failed; each replaces the previous one.
  - Queued by the reducer once it accepts an event, as high-priority jobs that skip concurrency caps.

## Consequences
- No polling for reactions.
- Acknowledgements depend on the reducer and executor being healthy, not just the receiver. That's intended: 👀 means accepted, not just delivered.
