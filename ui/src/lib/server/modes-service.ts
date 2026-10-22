import type { ModeIssue, ModesDoc, ModesFile } from '#lib/modes-types.ts';
import type { ProjectView } from '#lib/project-types.ts';
import {
	commitDesignerFile,
	conflictOnOpen,
	designerAccess,
	fail as failWith,
	readDesignerFiles,
	type Actor,
	type DesignerDeps,
	type Failure as DesignerFailure
} from './designer-files.ts';
import { TracksError, type TracksClient } from './tracks.ts';

export { projectRepoKeys, type Actor } from './designer-files.ts';

/**
 * The modes designer's server side (ADR-0029 §4, §7): `.ez/modes.toml` is read from and committed
 * to the repository's default branch by the enzarb App, for users with write access right now.
 * Validation and rendering are tracks' (ez-core); this file only moves text and checks access.
 */

export const MODES_PATH = '.ez/modes.toml';
const EMPTY: ModesDoc = { defaultMode: '', modes: [] };

export interface ModesDeps extends DesignerDeps {
	tracks: TracksClient;
}

export type Failure = DesignerFailure<ModeIssue>;
const fail = (code: Failure['code'], message: string, issues?: ModeIssue[]): Failure =>
	failWith(code, message, issues);

function tracksFailure(e: unknown): Failure {
	if (e instanceof TracksError) {
		if (e.code === 'invalid_modes') return fail('invalid', 'The modes have errors.', e.issues);
		if (e.code === 'invalid_modes_toml') return fail('invalid', e.message);
		return fail('tracks', 'The modes service (enzarb-tracks) is not available.');
	}
	throw e;
}

async function readCurrent(deps: ModesDeps, name: string, branch: string) {
	const r = await readDesignerFiles(deps, name, branch, [MODES_PATH]);
	return r.ok ? { ok: true as const, file: r.files[0] } : r;
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
	const a = await designerAccess(deps, actor.login, project, requested);
	if (!a.ok) return a;
	const name = a.name;
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

export const DEFAULT_COMMIT_SUBJECT = 'chore(ez): update .ez/modes.toml from enzarb';

export async function saveModes(
	deps: ModesDeps,
	actor: Actor,
	project: Pick<ProjectView, 'repositories'>,
	input: SaveInput
): Promise<SaveResult> {
	const a = await designerAccess(deps, actor.login, project, input.repo);
	if (!a.ok) return a;
	const name = a.name;
	const branch = a.repo.defaultBranch;
	const cur = await readCurrent(deps, name, branch);
	if (!cur.ok) return cur;
	const currentSha = cur.file?.sha ?? null;
	if (currentSha !== input.baseSha) return conflictOnOpen(MODES_PATH, branch);

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

	const subject = input.message?.trim() || DEFAULT_COMMIT_SUBJECT;
	const r = await commitDesignerFile(deps, actor, a, {
		path: MODES_PATH,
		text,
		sha: currentSha,
		subject,
		designer: 'modes designer'
	});
	return r.ok ? { ...r, unchanged: false, text } : r;
}
