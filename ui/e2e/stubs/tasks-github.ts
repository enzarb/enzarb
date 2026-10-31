// Stand-in for src/lib/server/tasks-github.ts in the e2e web server: one `ez/*` pull request and
// one triaged issue in the e2e repository, without GitHub.
import type { GithubReads } from '../../src/lib/server/tasks-github.ts';

export * from '../../src/lib/server/tasks-github.ts';

const reads: GithubReads = {
	openSubjects: async () => ({ open: null }),
	pulls: async (repo) => ({
		items:
			repo === 'e2e-org/app'
				? [
						{
							number: 50,
							title: 'Fix #7: crash on start',
							url: 'https://github.com/e2e-org/app/pull/50',
							branch: 'ez/fix-issue/11111111-1111-4111-8111-111111111111',
							draft: false,
							updatedAt: '2026-10-30T12:05:00.000Z'
						}
					]
				: []
	}),
	issues: async (repo) => ({
		items:
			repo === 'e2e-org/app'
				? [
						{
							number: 9,
							title: 'Triaged bug',
							body: 'The app crashes on start.',
							url: 'https://github.com/e2e-org/app/issues/9',
							updatedAt: '2026-10-30T12:06:00.000Z'
						}
					]
				: []
	})
};

export const createGithubReads = (): GithubReads => reads;
