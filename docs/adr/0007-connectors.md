# 0007. Source-agnostic connectors

Status: **Superseded** by [ADR-0027](0027-workflow-clean-slate.md). The Connector trait, the normalized event catalog and source-neutral work item keys are gone; the text below is the original intent.

## Context
GitHub is the first system that sends events to enzarb, but GitLab, Forgejo, Jira, Slack and others were expected.

## Decision (original)
- A `connectors` component: a `Connector` trait, one connector per source, and a webhook `receiver` binary.
- Inbound payloads normalized into a fixed set of events, using CloudEvents attributes.
- The name avoids terms that already mean something in Kubernetes: ingress, gateway, router, proxy, controller, scheduler.

## Today
`connectors/` is only the `receiver`: it verifies the GitHub signature and logs each delivery in `connectors.events`. See [connectors.md](../connectors.md).
