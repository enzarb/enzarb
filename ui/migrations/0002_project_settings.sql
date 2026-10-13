-- Per-Project UI settings, keyed by Project name (the CRD lives in Kubernetes, so no FK).
CREATE TABLE app.project_settings (
	project                  text        PRIMARY KEY,
	claude_token_configured  boolean     NOT NULL DEFAULT false,
	claude_configured_at     timestamptz,
	claude_configured_by     text,
	updated_at               timestamptz NOT NULL DEFAULT now()
);

-- Default privileges from 0001 grant the runtime role access to this table.
