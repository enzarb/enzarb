// Stand-in for src/lib/server/tasks-github-writes.ts in the e2e web server: approvals and merges
// without GitHub. Merging marks the pull request merged so it leaves the open list.
import type { GithubWrites } from '../../src/lib/server/tasks-github-writes.ts';
import { merged } from './tasks-github.ts';

export * from '../../src/lib/server/tasks-github-writes.ts';

/** What the stub was asked to write, newest first. */
export const writes: string[] = [];

const stub: GithubWrites = {
	approve: async (_repo, n, body) => {
		writes.unshift(`approve #${n}: ${body}`);
		return { ok: true };
	},
	comment: async (_repo, n, body) => {
		writes.unshift(`comment #${n}: ${body}`);
		return { ok: true };
	},
	squashMerge: async (_repo, n, { title }) => {
		writes.unshift(`merge #${n}: ${title}`);
		merged.add(n);
		return { ok: true };
	}
};

export const createGithubWrites = (): GithubWrites => stub;
