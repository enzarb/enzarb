// Stand-in for src/lib/server/repo-access.ts in the e2e web server (see vite.e2e.config.ts): the
// e2e user may write to the fixed E2E_REPOS, without GitHub.
import type { RepositoryAccess, UserRepository } from '../../src/lib/server/user-repositories.ts';
import { E2E_REPOS } from '../constants.ts';

const repos: UserRepository[] = E2E_REPOS.map((repo) => ({
	repo,
	installationId: 1,
	accountLogin: 'e2e-org',
	private: false,
	defaultBranch: 'main'
}));

const access: RepositoryAccess = {
	configured: true,
	list: async () => ({ repositories: repos, fetchedAt: new Date() }),
	check: async (_login, key) => repos.find((r) => r.repo === key) ?? null
};

export const getRepositoryAccess = (): RepositoryAccess => access;
