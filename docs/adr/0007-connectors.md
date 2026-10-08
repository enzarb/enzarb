# 0007. Source-agnostic connectors

## Context
GitHub is the first system that triggers workflows, but GitLab, Forgejo, Jira, Slack and others are expected.

## Decision
- A new `enzarb-connectors` repo: a `Connector` trait, one connector per source, and a webhook `receiver` binary.
- Inbound payloads are normalized into a fixed set of events (`WorkItemOpened`, `CommentAdded`, `Mentioned`, `ApprovalGranted`, `ChangeRequestOpened`, `ChangeRequestUpdated`, `ReviewSubmitted`, `WorkItemClosed`) using CloudEvents attributes.
- Work items use source-neutral keys (`github:org/repo#123`).
- Workflows never import a provider SDK.
- The name avoids terms that already mean something in Kubernetes: ingress, gateway, router, proxy, controller, scheduler.

See [connectors.md](../connectors.md).

## Consequences
- Each new source means one connector, plus the trait conformance suite.
- Behaviour specific to a provider (such as how approval is detected) stays inside its connector.
