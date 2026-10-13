-- Snapshot of the repositories a user can reach through installations of the enzarb GitHub App,
-- replaced for that user on each GitHub sign-in. No tokens are stored.
-- repo is the repository key `github:<owner>/<repo>`, lowercase.
CREATE TABLE app.user_repositories (
	user_id         bigint      NOT NULL REFERENCES app.users (id) ON DELETE CASCADE,
	repo            text        NOT NULL,
	installation_id bigint      NOT NULL,
	account_login   text        NOT NULL,
	private         boolean     NOT NULL,
	default_branch  text        NOT NULL,
	fetched_at      timestamptz NOT NULL DEFAULT now(),
	PRIMARY KEY (user_id, repo)
);

-- Default privileges from 0001 grant the runtime role access to this table.
