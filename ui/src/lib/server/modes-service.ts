import type { ModeIssue, ModesDoc, ModesFile } from '#lib/modes-types.ts';
import type { ProjectView } from '#lib/project-types.ts';
import type { RepoPermissions } from './github-app.ts';
import { coAuthorTrailer, commitRepoFile, readRepoFile } from './repo-files.ts';
import { TracksError, type TracksClient } from './tracks.ts';
import type { UserRepository } from './user-repositories.ts';

/**
 * The modes designer's server side (ADR-0029 §4, §7): `.ez/modes.toml` is read from and committed
 * to the repository's default branch by the enzarb App, for users with write access right now.
 * Validation and rendering are tracks' (ez-core); this file only moves text and checks access.
 */

export const MODES_PATH = '.ez/modes.toml';
const READ: RepoPermissions = { contents: 'read' };
const WRITE: RepoPermissions = { contents: 'write' };
const EMPTY: ModesDoc = { defaultMode: '', modes: [] };

export interface ModesDeps {
	tracks: TracksClient;
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

const fullName = (key: string) => key.replace(/^github:/, '');

export type Failure = {
	ok: false;
	code:
		'no_repository' | 'forbidden' | 'not_installed' | 'github' | 'tracks' | 'conflict' | 'invalid';
	message: string;
	issues?: ModeIssue[];
};

const fail = (code: Failure['code'], message: string, issues?: ModeIssue[]): Failure => ({
	ok: false,
	code,
	message,
	...(issues ? { issues } : {})
});

function tracksFailure(e: unknown): Failure {
	if (e instanceof TracksError) {
		if (e.code === 'invalid_modes') return fail('invalid', 'The modes have errors.', e.issues);
		if (e.code === 'invalid_modes_toml') return fail('invalid', e.message);
		return fail('tracks', 'The modes service (enzarb-tracks) is not available.');
	}
	throw e;
}

/** Repository chosen (`requested`, default the first) among the Project's, with write access. */
async function access(
	deps: ModesDeps,
	login: string,
	project: Pick<ProjectView, 'repositories'>,
	requested: string | undefined
): Promise<{ ok: true; keys: string[]; key: string; repo: UserRepository } | Failure> {
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
	return { ok: true, keys, key, repo };
}

async function readCurrent(deps: ModesDeps, name: string, branch: string) {
	const token = await deps.token(name, READ);
	if (!token) return fail('not_installed', `The enzarb App cannot read ${name}.`);
	const r = await readRepoFile({
		repo: name,
		path: MODES_PATH,
		ref: branch,
		token,
		fetch: deps.fetch
	});
	if (!r.ok) {
		if (r.status === 401) deps.invalidate(name, READ);
		return fail('github', `Could not read ${MODES_PATH} from ${name}: ${r.message}.`);
	}
	return { ok: true as const, file: r.file };
}

export type ModesLoad = ModesFile & {
	ok: true;
	/** `github:owner/repo` keys the designer can switch between. */
	repos: string[];
};

export async function loadModes(
	deps: ModesDeps,
	actor: Actor,
	project: Pick<ProjectView, 'repositories'>,
	requested?: string
): Promise<ModesLoad | Failure> {
	const a = await access(deps, actor.login, project, requested);
	if (!a.ok) return a;
	const name = fullName(a.key);
	const branch = a.repo.defaultBranch;
	const cur = await readCurrent(deps, name, branch);
	if (!cur.ok) return cur;
	try {
		const catalog = await deps.tracks.modesCatalog();
		const base = {
			ok: true as const,
			repos: a.keys,
			repo: a.key,
			branch,
			path: MODES_PATH,
			catalog
		};
		if (!cur.file)
			return { ...base, sha: null, text: null, doc: EMPTY, issues: [], parseError: null };
		try {
			const { doc, issues } = await deps.tracks.parseModes(cur.file.text);
			return { ...base, sha: cur.file.sha, text: cur.file.text, doc, issues, parseError: null };
		} catch (e) {
			if (e instanceof TracksError && e.code === 'invalid_modes_toml')
				return {
					...base,
					sha: cur.file.sha,
					text: cur.file.text,
					doc: EMPTY,
					issues: [],
					parseError: e.message
				};
			throw e;
		}
	} catch (e) {
		return tracksFailure(e);
	}
}

export interface SaveInput {
	repo: string;
	/** Blob sha the edit started from; null when the file did not exist. */
	baseSha: string | null;
	/** Exactly one: the form's document (rendered onto the current file) or raw TOML text. */
	doc?: ModesDoc;
	text?: string;
	/** Commit subject; a default is used when empty. */
	message?: string;
}

export type SaveResult =
	| { ok: true; unchanged: boolean; sha: string | null; commitUrl: string | null; text: string }
	| Failure;

export const DEFAULT_COMMIT_SUBJECT = 'chore(ez): update .ez/modes.toml';

export async function saveModes(
	deps: ModesDeps,
	actor: Actor,
	project: Pick<ProjectView, 'repositories'>,
	input: SaveInput
): Promise<SaveResult> {
	const a = await access(deps, actor.login, project, input.repo);
	if (!a.ok) return a;
	const name = fullName(a.key);
	const branch = a.repo.defaultBranch;
	const cur = await readCurrent(deps, name, branch);
	if (!cur.ok) return cur;
	const currentSha = cur.file?.sha ?? null;
	if (currentSha !== input.baseSha)
		return fail(
			'conflict',
			`${MODES_PATH} changed on ${branch} since you opened it. Reload to get the latest version; your edits are not saved.`
		);

	let text: string;
	try {
		if (input.doc) {
			text = (await deps.tracks.renderModes(input.doc, cur.file?.text ?? null)).text;
		} else if (input.text !== undefined) {
			const { issues } = await deps.tracks.parseModes(input.text);
			if (issues.some((i) => i.severity === 'error'))
				return fail('invalid', 'The modes have errors.', issues);
			text = input.text;
		} else {
			return fail('invalid', 'Nothing to save.');
		}
	} catch (e) {
		return tracksFailure(e);
	}
	if (cur.file && text === cur.file.text)
		return { ok: true, unchanged: true, sha: currentSha, commitUrl: null, text };

	const token = await deps.token(name, WRITE);
	if (!token) return fail('not_installed', `The enzarb App cannot write to ${name}.`);
	const subject = input.message?.trim() || DEFAULT_COMMIT_SUBJECT;
	const r = await commitRepoFile({
		repo: name,
		path: MODES_PATH,
		branch,
		text,
		sha: currentSha,
		message: `${subject}\n\nEdited in the enzarb modes designer.\n\n${coAuthorTrailer(actor)}\n`,
		token,
		fetch: deps.fetch
	});
	if (r.ok) return { ok: true, unchanged: false, sha: r.sha, commitUrl: r.commitUrl, text };
	if (r.status === 401) deps.invalidate(name, WRITE);
	if (r.reason === 'conflict')
		return fail(
			'conflict',
			`${MODES_PATH} changed on ${branch} while saving. Reload and try again.`
		);
	if (r.reason === 'forbidden')
		return fail(
			'forbidden',
			`GitHub refused the commit to ${branch}. The branch may be protected against the enzarb App.`
		);
	return fail('github', `Could not commit to ${name}: ${r.message}.`);
}
