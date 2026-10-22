# 0005. SvelteKit remote functions as the UI logic layer

## Decision
- SvelteKit (adapter-node) **remote functions are the logic layer**, not thin wrappers around another API.
- Components stay deliberately small. Styling is one global `global.scss`, with no CSS frameworks.
- Long operations show **step-by-step checklists** backed by real status conditions, never open-ended spinners.

## Consequences
- The UI talks to Postgres (its own `app` schema, plus the `connectors.event_v` view) and to the Kubernetes API for `Project` CRs. It has no separate backend.
- Remote functions are still behind an experimental flag; versions are pinned.
