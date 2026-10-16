-- Deprecated (issue #227): the repository list is now read live from GitHub through the enzarb
-- GitHub App, and access is checked live when a repository is added to a Project. The UI no longer
-- reads or writes app.user_repositories. It is kept (not dropped) so a replica of the previous
-- release still runs during a rollout; a later migration may drop it.
COMMENT ON TABLE app.user_repositories IS
	'Deprecated, unused since ui issue #227 (live repository list). Safe to drop in a later release.';
