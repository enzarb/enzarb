## Review

1. **Must fix:** `docs/github-app.yml` adds `deployment_status` to `default_events`, but `default_permissions` has no `deployments: read`. GitHub requires the matching permission for each subscribed event, so the manifest flow will likely reject the manifest or silently drop the event. Add `deployments: read`.

Optional: `release.published` is the only new behaviour. It is consistent with `phases/release.yaml` and the connector catalog (`catalog.rs:328`). Existing installed Apps must be updated by hand to subscribe to `release`; the manifest only affects new Apps, so mention that in the PR or in `docs/dev.md`.

Only YAML and docs changed. I ran no component checks and did not run `enzarb-lint`; I am relying on the PR description for that.
