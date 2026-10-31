/** Readable names for the GitHub subjects and enzarb runs that Tasks, Runs and Events refer to. */

/** Which repositories a page deals with, to decide how much of `owner/repo` to show. */
export interface RepoContext {
	/** The owner most of the repositories belong to (the Project's). */
	owner: string;
	/** Every `owner/repo` on the page. */
	repos: string[];
}

const ownerOf = (repo: string) => repo.split('/')[0] ?? '';
const nameOf = (repo: string) => repo.split('/').slice(1).join('/') || repo;
const lower = (s: string) => s.toLowerCase();

/** The context of a set of repositories: the most common owner wins, ties go to the first. */
export function repoContext(repos: string[]): RepoContext {
	const count = new Map<string, number>();
	for (const r of repos) count.set(lower(ownerOf(r)), (count.get(lower(ownerOf(r))) ?? 0) + 1);
	let owner = '';
	let best = 0;
	for (const r of repos) {
		const n = count.get(lower(ownerOf(r))) ?? 0;
		if (n > best) {
			best = n;
			owner = ownerOf(r);
		}
	}
	return { owner, repos: [...new Set(repos)] };
}

/**
 * `enzarb#43` when the repository is the Project owner's and its name is unique on the page;
 * `owner/repo#43` otherwise (another owner, or two repositories with the same name).
 */
export function subjectLabel(repo: string, number: number | null, ctx?: RepoContext): string {
	const suffix = number ? `#${number}` : '';
	const short =
		!!ctx &&
		lower(ownerOf(repo)) === lower(ctx.owner) &&
		!ctx.repos.some((r) => lower(r) !== lower(repo) && lower(nameOf(r)) === lower(nameOf(repo)));
	return `${short ? nameOf(repo) : repo}${suffix}`;
}

/** `review-pull-requests #1a2b3c4d` */
export function runLabel(workflow: string, id: string): string {
	return `${workflow} #${id.slice(0, 8)}`;
}

/** The GitHub URL of an issue (GitHub redirects pull requests from /issues/). */
export const subjectUrl = (repo: string, number: number) =>
	`https://github.com/${repo}/issues/${number}`;
