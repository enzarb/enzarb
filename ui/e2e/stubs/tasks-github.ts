// Stand-in for src/lib/server/tasks-github.ts in the e2e web server: one `ez/*` pull request and
// one triaged issue in the e2e repository, without GitHub.
import type { GithubReads } from '../../src/lib/server/tasks-github.ts';

export * from '../../src/lib/server/tasks-github.ts';

/** Pull requests the stub has merged (they leave the open list). */
export const merged = new Set<number>();

const PATCH = [
	'@@ -1,4 +1,5 @@',
	' export function start() {',
	'-\treturn boot(null);',
	'+\tif (!config) return;',
	'+\treturn boot(config);',
	' }',
	' // end'
].join('\n');

const FILES = [
	{ filename: 'src/start.ts', status: 'modified', additions: 2, deletions: 1, patch: PATCH },
	{ filename: 'assets/logo.png', status: 'added', additions: 0, deletions: 0 },
	{ filename: 'src/huge.generated.ts', status: 'added', additions: 9000, deletions: 0 }
];

const pullState = (n: number) => ({
	number: n,
	title: n === 50 ? 'Fix #7: crash on start' : 'Broken change',
	url: `https://github.com/e2e-org/app/pull/${n}`,
	draft: false,
	headSha: `sha${n}`,
	mergeable: true,
	mergeableState: n === 50 ? 'clean' : 'unstable'
});

const reads: GithubReads = {
	pull: async (_repo, n) => ({ pull: n === 50 || n === 51 ? pullState(n) : null }),
	pullFiles: async () => ({ items: FILES }),
	checkRuns: async (_repo, sha) => ({
		items:
			sha === 'sha51'
				? [
						{ name: 'build', status: 'completed', conclusion: 'success', url: '' },
						{ name: 'lint', status: 'completed', conclusion: 'failure', url: '' }
					]
				: [
						{ name: 'build', status: 'completed', conclusion: 'success', url: '' },
						{ name: 'test', status: 'completed', conclusion: 'success', url: '' }
					]
	}),
	markedComments: async () => ({ items: [] }),
	openSubjects: async () => ({ open: null }),
	pulls: async (repo) => ({
		items:
			repo === 'e2e-org/app'
				? [
						{
							number: 51,
							title: 'Broken change',
							url: 'https://github.com/e2e-org/app/pull/51',
							branch: 'ez/fix-issue/22222222-2222-4222-8222-222222222222',
							draft: false,
							updatedAt: '2026-10-30T12:04:00.000Z'
						},
						{
							number: 50,
							title: 'Fix #7: crash on start',
							url: 'https://github.com/e2e-org/app/pull/50',
							branch: 'ez/fix-issue/11111111-1111-4111-8111-111111111111',
							draft: false,
							updatedAt: '2026-10-30T12:05:00.000Z'
						}
					].filter((p) => !merged.has(p.number))
				: []
	}),
	issues: async (repo) => ({
		items:
			repo === 'e2e-org/app'
				? [
						{
							number: 9,
							title: 'Triaged bug',
							bodyTruncated: false,
							body: 'The app crashes on start.',
							url: 'https://github.com/e2e-org/app/issues/9',
							updatedAt: '2026-10-30T12:06:00.000Z'
						}
					]
				: []
	})
};

export const createGithubReads = (): GithubReads => reads;
