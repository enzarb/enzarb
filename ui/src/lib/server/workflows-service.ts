import type { ProjectView } from '#lib/project-types.ts';
import {
	hasErrorDiagnostics,
	type Diagnostic,
	type DryRunOptions,
	type DryRunResponse,
	type WorkflowsFile,
	type WorkflowsFileWire
} from '#lib/workflows-types.ts';
import {
	commitDesignerFile,
	conflictOnOpen,
	designerAccess,
	fail as failWith,
	projectRepoKeys,
	readDesignerFiles,
	type Actor,
	type DesignerDeps,
	type Failure as DesignerFailure
} from './designer-files.ts';
import { MODES_PATH } from './modes-service.ts';
import { TracksError, type TracksClient } from './tracks.ts';

/**
 * The workflow designer's server side (ADR-0029 §2, §6, §7). `.ez/workflows.toml` lives on the
 * repository's default branch next to `.ez/modes.toml`, which every check is made against. Saving
 * makes the workflows live, so a file with errors is never committed. Validation, rendering and
 * dry runs are tracks' (ez-workflow).
 */

export const WORKFLOWS_PATH = '.ez/workflows.toml';
export const DEFAULT_WORKFLOWS_SUBJECT = 'chore(ez): update .ez/workflows.toml';
export const MAX_DRY_RUN_EVENTS = 50;

export interface WorkflowsDeps extends DesignerDeps {
	tracks: TracksClient;
}

export type Failure = DesignerFailure<Diagnostic>;
const fail = (code: Failure['code'], message: string, issues?: Diagnostic[]): Failure =>
	failWith(code, message, issues);

function tracksFailure(e: unknown): Failure {
	if (e instanceof TracksError) {
		if (e.status === 422 || e.status === 400) return fail('invalid', e.message);
		return fail('tracks', 'The workflows service (enzarb-tracks) is not available.');
	}
	throw e;
}

/** Mode ids of a modes file, for the agent node's picker (empty when it does not parse). */
async function modeIds(tracks: TracksClient, modes: string | null): Promise<string[]> {
	if (modes === null) return [];
	try {
		return (await tracks.parseModes(modes)).doc.modes.map((m) => m.id);
	} catch (e) {
		if (e instanceof TracksError && e.code === 'invalid_modes_toml') return [];
		throw e;
	}
}

export type WorkflowsLoad = WorkflowsFile & { ok: true; repos: string[] };

export async function loadWorkflows(
	deps: WorkflowsDeps,
	actor: Actor,
	project: Pick<ProjectView, 'repositories'>,
	requested?: string
): Promise<WorkflowsLoad | Failure> {
	const a = await designerAccess(deps, actor.login, project, requested);
	if (!a.ok) return a;
	const branch = a.repo.defaultBranch;
	const r = await readDesignerFiles(deps, a.name, branch, [WORKFLOWS_PATH, MODES_PATH]);
	if (!r.ok) return r;
	const [wf, modes] = r.files;
	const modesToml = modes?.text ?? null;
	try {
		const ids = await modeIds(deps.tracks, modesToml);
		const base = {
			ok: true as const,
			repos: a.keys,
			repo: a.key,
			branch,
			path: WORKFLOWS_PATH,
			modesToml,
			modeIds: ids
		};
		if (!wf) return { ...base, sha: null, text: null, file: { version: 1 }, report: [] };
		const { diagnostics, file } = await deps.tracks.checkWorkflows(wf.text, modesToml);
		return { ...base, sha: wf.sha, text: wf.text, file, report: diagnostics };
	} catch (e) {
		return tracksFailure(e);
	}
}

export interface SaveWorkflowsInput {
	repo: string;
	baseSha: string | null;
	/** Exactly one: the designer's document (re-serialized; comments are lost) or raw TOML. */
	file?: WorkflowsFileWire;
	text?: string;
	message?: string;
}

export type SaveWorkflowsResult =
	| {
			ok: true;
			unchanged: boolean;
			sha: string | null;
			commitUrl: string | null;
			text: string;
			report: Diagnostic[];
	  }
	| Failure;

export async function saveWorkflows(
	deps: WorkflowsDeps,
	actor: Actor,
	project: Pick<ProjectView, 'repositories'>,
	input: SaveWorkflowsInput
): Promise<SaveWorkflowsResult> {
	const a = await designerAccess(deps, actor.login, project, input.repo);
	if (!a.ok) return a;
	const branch = a.repo.defaultBranch;
	const r = await readDesignerFiles(deps, a.name, branch, [WORKFLOWS_PATH, MODES_PATH]);
	if (!r.ok) return r;
	const [cur, modes] = r.files;
	const currentSha = cur?.sha ?? null;
	if (currentSha !== input.baseSha) return conflictOnOpen(WORKFLOWS_PATH, branch);

	// Checked against the modes on the branch now, not the ones the designer loaded.
	const modesToml = modes?.text ?? null;
	let text: string;
	let report: Diagnostic[];
	try {
		if (input.file) {
			({ text, diagnostics: report } = await deps.tracks.renderWorkflows(input.file, modesToml));
		} else if (input.text !== undefined) {
			text = input.text;
			report = (await deps.tracks.checkWorkflows(text, modesToml)).diagnostics;
		} else {
			return fail('invalid', 'Nothing to save.');
		}
	} catch (e) {
		return tracksFailure(e);
	}
	if (hasErrorDiagnostics(report))
		return fail('invalid', 'The workflows have errors; saving would make them live.', report);
	if (cur && text === cur.text)
		return { ok: true, unchanged: true, sha: currentSha, commitUrl: null, text, report };

	const c = await commitDesignerFile(deps, actor, a, {
		path: WORKFLOWS_PATH,
		text,
		sha: currentSha,
		subject: input.message?.trim() || DEFAULT_WORKFLOWS_SUBJECT,
		designer: 'workflow designer'
	});
	return c.ok ? { ...c, unchanged: false, text, report } : c;
}

export interface LoggedEvent {
	id: number;
	event: string;
	action: string | null;
	payload: unknown;
}

export interface DryRunDeps {
	tracks: TracksClient;
	/** The events with these ids that belong to `repo` (`owner/repo`), any order. */
	events: (repo: string, ids: number[]) => Promise<LoggedEvent[]>;
}

export type DryRunLoad = DryRunResponse & { ok: true; missing: number[] };

/**
 * Dry-runs unsaved workflows against events logged for one of the Project's repositories. No
 * GitHub access is needed: nothing is read from or written to the repository.
 */
export async function dryRunWorkflows(
	deps: DryRunDeps,
	project: Pick<ProjectView, 'repositories'>,
	input: {
		repo: string;
		text: string;
		modesToml: string | null;
		eventIds: number[];
		options?: DryRunOptions;
	}
): Promise<DryRunLoad | Failure> {
	const keys = projectRepoKeys(project);
	const key = input.repo.toLowerCase();
	if (!keys.includes(key))
		return fail('no_repository', 'This project has no such GitHub repository.');
	const ids = [...new Set(input.eventIds)];
	if (ids.length === 0 || ids.length > MAX_DRY_RUN_EVENTS)
		return fail('invalid', `Choose 1-${MAX_DRY_RUN_EVENTS} events.`);
	const rows = await deps.events(key.replace(/^github:/, ''), ids);
	const byId = new Map(rows.map((e) => [e.id, e]));
	const events = ids.flatMap((id) => {
		const e = byId.get(id);
		return e ? [e] : [];
	});
	const missing = ids.filter((id) => !byId.has(id));
	if (events.length === 0)
		return fail('invalid', 'None of these events were logged for this repository.');
	try {
		const r = await deps.tracks.dryRun({
			text: input.text,
			modes: input.modesToml,
			events,
			options: input.options
		});
		return { ok: true, ...r, missing };
	} catch (e) {
		if (e instanceof TracksError && e.status === 413)
			return fail('invalid', 'These events are too large to send together; choose fewer.');
		return tracksFailure(e);
	}
}
