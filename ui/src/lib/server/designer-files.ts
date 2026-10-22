import type { ProjectView } from '#lib/project-types.ts';
import type { RepoPermissions } from './github-app.ts';
import { coAuthorTrailer, commitRepoFile, readRepoFile, type RepoFile } from './repo-files.ts';
import type { UserRepository } from './user-repositories.ts';

/**
 * What the modes and workflow designers share (ADR-0029 §7): the repository is one of the
 * Project's, the user has write access on it right now, and files are read from and committed to
 * its live default branch by the enzarb App.
 */

const READ: RepoPermissions = { contents: 'read' };
const WRITE: RepoPermissions = { contents: 'write' };

export interface DesignerDeps {
	/** Live write-permission check (`RepositoryAccess.check`). */
	checkAccess: (login: string, key: string) => Promise<UserRepository | null>;
	/** Installation token for `owner/repo` (`github-app.token`). */
	token: (repo: string, perms: RepoPermissions) => Promise<string | null>;
	invalidate: (repo: string, perms: RepoPermissions) => void;
	fetch?: typeof fetch;
}

export interface Actor {
	login: string;
	githubId: number;
}

/** `github:owner/repo` keys of the Project's repositories, in spec order. */
export function projectRepoKeys(project: Pick<ProjectView, 'repositories'>): string[] {
	const out: string[] = [];
	for (const r of project.repositories) {
		const m = /^https:\/\/github\.com\/([\w.-]+\/[\w.-]+?)(?:\.git)?\/?$/.exec(r.url);
		if (m) out.push(`github:${m[1].toLowerCase()}`);
	}
	return out;
}

export const fullName = (key: string) => key.replace(/^github:/, '');

export type FailureCode =
	'no_repository' | 'forbidden' | 'not_installed' | 'github' | 'tracks' | 'conflict' | 'invalid';

export type Failure<I = never> = {
	ok: false;
	code: FailureCode;
	message: string;
	issues?: I[];
};

export function fail<I = never>(code: FailureCode, message: string, issues?: I[]): Failure<I> {
	return { ok: false, code, message, ...(issues ? { issues } : {}) };
}

export type Access = { ok: true; keys: string[]; key: string; name: string; repo: UserRepository };

/** Repository chosen (`requested`, default the first) among the Project's, with write access. */
export async function designerAccess(
	deps: DesignerDeps,
	login: string,
	project: Pick<ProjectView, 'repositories'>,
	requested: string | undefined
): Promise<Access | Failure> {
	const keys = projectRepoKeys(project);
	const key = requested ? requested.toLowerCase() : keys[0];
	if (!key || !keys.includes(key))
		return fail('no_repository', 'This project has no such GitHub repository.');
	const repo = await deps.checkAccess(login, key);
	if (!repo)
		return fail(
			'forbidden',
			`You need write access on ${fullName(key)}, and the enzarb App must be installed on it.`
		);
	return { ok: true, keys, key, name: fullName(key), repo };
}

/** Reads `paths` from `branch` of `name` with one read token (null per missing file). */
export async function readDesignerFiles(
	deps: DesignerDeps,
	name: string,
	branch: string,
	paths: string[]
): Promise<{ ok: true; files: (RepoFile | null)[] } | Failure> {
	const token = await deps.token(name, READ);
	if (!token) return fail('not_installed', `The enzarb App cannot read ${name}.`);
	const files: (RepoFile | null)[] = [];
	for (const path of paths) {
		const r = await readRepoFile({ repo: name, path, ref: branch, token, fetch: deps.fetch });
		if (!r.ok) {
			if (r.status === 401) deps.invalidate(name, READ);
			return fail('github', `Could not read ${path} from ${name}: ${r.message}.`);
		}
		files.push(r.file);
	}
	return { ok: true, files };
}

export const conflictOnOpen = (path: string, branch: string) =>
	fail(
		'conflict',
		`${path} changed on ${branch} since you opened it. Reload to get the latest version; your edits are not saved.`
	);

export type Committed = { ok: true; sha: string | null; commitUrl: string | null };

/** Commits `text` to `path` on the default branch, with the actor as co-author. */
export async function commitDesignerFile(
	deps: DesignerDeps,
	actor: Actor,
	a: Access,
	input: { path: string; text: string; sha: string | null; subject: string; designer: string }
): Promise<Committed | Failure> {
	const { name } = a;
	const branch = a.repo.defaultBranch;
	const token = await deps.token(name, WRITE);
	if (!token) return fail('not_installed', `The enzarb App cannot write to ${name}.`);
	const r = await commitRepoFile({
		repo: name,
		path: input.path,
		branch,
		text: input.text,
		sha: input.sha,
		message: `${input.subject}\n\nEdited in the enzarb ${input.designer}.\n\n${coAuthorTrailer(actor)}\n`,
		token,
		fetch: deps.fetch
	});
	if (r.ok) return { ok: true, sha: r.sha, commitUrl: r.commitUrl };
	if (r.status === 401) deps.invalidate(name, WRITE);
	if (r.reason === 'conflict')
		return fail(
			'conflict',
			`${input.path} changed on ${branch} while saving. Reload and try again.`
		);
	if (r.reason === 'forbidden')
		return fail(
			'forbidden',
			`GitHub refused the commit to ${branch}. The branch may be protected against the enzarb App.`
		);
	return fail('github', `Could not commit to ${name}: ${r.message}.`);
}
