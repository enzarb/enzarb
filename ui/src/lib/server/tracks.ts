import { getConfig, type TracksConfig } from './config.ts';
import type { EventDecision } from '#lib/events.ts';
import type { NodeRun, RunDetail, RunSummary } from '#lib/runs.ts';
import type { ModeIssue, ModesCatalog, ModesDoc } from '#lib/modes-types.ts';
import type {
	Diagnostic,
	DryRunOptions,
	DryRunResponse,
	WorkflowsFileWire
} from '#lib/workflows-types.ts';

/**
 * Client for enzarb-tracks (cluster-internal, ADR-0029; API in tracks/README.md). The UI calls it
 * only after its own access checks. Every rule about modes and workflows lives in tracks (ez-core,
 * ez-workflow): this file never re-derives a validation.
 */

const TIMEOUT_MS = 10_000;
/** A dry run renders up to 50 events' traces. */
const DRY_RUN_TIMEOUT_MS = 30_000;

/** tracks answered with `{"error": {code, message}}` (or could not be reached: `unavailable`). */
export class TracksError extends Error {
	readonly status: number;
	readonly code: string;
	/** Validation issues, on 422 `invalid_modes`. */
	readonly issues: ModeIssue[];

	constructor(status: number, code: string, message: string, issues: ModeIssue[] = []) {
		super(message);
		this.name = 'TracksError';
		this.status = status;
		this.code = code;
		this.issues = issues;
	}
}

export interface TracksClient {
	modesCatalog(): Promise<ModesCatalog>;
	/** Parse `.ez/modes.toml`; 422 `invalid_modes_toml` when it is not valid TOML or not modes. */
	parseModes(text: string): Promise<{ doc: ModesDoc; issues: ModeIssue[] }>;
	validateModes(doc: ModesDoc): Promise<ModeIssue[]>;
	/** The file for `doc`, editing `old` in place (comments kept); 422 `invalid_modes` on errors. */
	renderModes(
		doc: ModesDoc,
		old: string | null
	): Promise<{ text: string; issues: ModeIssue[]; rewritten: string[] }>;
	/** Parse and validate `.ez/workflows.toml` (against `modes`, the same commit's modes file). */
	checkWorkflows(
		text: string,
		modes: string | null
	): Promise<{ diagnostics: Diagnostic[]; file: WorkflowsFileWire | null }>;
	/** The TOML for a designer document, with its report (comments are not kept). */
	renderWorkflows(
		file: WorkflowsFileWire,
		modes: string | null
	): Promise<{ text: string; diagnostics: Diagnostic[] }>;
	/** What routing decided about each event; events with no decision yet are absent. */
	decisions(eventIds: number[]): Promise<EventDecision[]>;
	/** Runs of a Project (newest first), or the runs the given events started. */
	runs(query: { project: string; eventIds?: number[] }): Promise<RunSummary[]>;
	/** One run with its node runs; null when it does not exist. */
	run(id: string): Promise<RunDetail | null>;
	/** Ask the executor to cancel the run; resolves false when it had already ended. */
	stopRun(id: string): Promise<boolean>;
	/**
	 * Start `workflow` on issue or pull request `number` of `repo` as `actor` (`POST /v1/runs`, a
	 * synthetic `manual` event; the workflow's `on` must list it). `alreadyRunning`: the subject
	 * already has a live run of the workflow (409), which is not an error for a button.
	 */
	startRun(input: {
		repo: string;
		number: number;
		workflow: string;
		actor: string;
	}): Promise<{ id: string | null; alreadyRunning: boolean }>;
	/**
	 * Deliver a synthetic `manual` event (`event.action` = `action`, `event.text` = `text`,
	 * `event.sender.login` = `actor`) to a run parked in a `wait` node (`POST /v1/runs/{id}/resume`).
	 * `ok`: resumed. `not_waiting` (409): the run is not waiting. `not_accepted` (422): the wait's
	 * `on`/`if` refuse this action.
	 */
	resumeRun(input: {
		id: string;
		actor: string;
		action: string;
		text?: string;
	}): Promise<{ result: 'ok' | 'not_waiting' | 'not_accepted'; message?: string }>;
	/** Triggers and traces of `text` for 1-50 logged events; no side effects. */
	dryRun(input: {
		text: string;
		modes: string | null;
		events: { id: number; event: string; action: string | null; payload: unknown }[];
		options?: DryRunOptions;
	}): Promise<DryRunResponse>;
}

interface WireDecision {
	event_id: number;
	outcome: EventDecision['outcome'];
	project: string | null;
	sha: string | null;
	matched: string[];
	reasons: { workflow: string; matched: boolean; on?: string | null; reason: string }[];
	errors: string[];
	resumed?: { run: string; workflow: string; node: string }[];
	decided_at: string;
}

interface WireRun {
	id: string;
	workflow: string;
	project: string;
	repo: string;
	subject: RunSummary['subject'];
	event_id: number;
	sha: string;
	status: RunSummary['status'];
	test: boolean;
	current_node: string | null;
	reason: string | null;
	stop_requested: boolean;
	created_at: string;
	updated_at: string;
	finished_at: string | null;
	wait_until?: string | null;
	node_runs?: {
		node: string;
		visit: number;
		status: string;
		rendered: NodeRun['rendered'];
		mode: string | null;
		outputs: NodeRun['outputs'];
		usage: NodeRun['usage'];
		git: NodeRun['git'];
		error: string | null;
		intent?: NodeRun['intent'];
		wait?: NodeRun['wait'];
		started_at: string | null;
		finished_at: string | null;
	}[];
}

const toRun = (r: WireRun): RunSummary => ({
	id: r.id,
	workflow: r.workflow,
	project: r.project,
	repo: r.repo,
	subject: r.subject,
	eventId: r.event_id,
	sha: r.sha,
	status: r.status,
	test: r.test,
	currentNode: r.current_node,
	reason: r.reason,
	stopRequested: r.stop_requested,
	createdAt: r.created_at,
	updatedAt: r.updated_at,
	finishedAt: r.finished_at,
	waitUntil: r.wait_until ?? null
});

const toDetail = (r: WireRun): RunDetail => ({
	...toRun(r),
	nodeRuns: (r.node_runs ?? []).map((n) => ({
		node: n.node,
		visit: n.visit,
		status: n.status,
		rendered: n.rendered,
		mode: n.mode,
		outputs: n.outputs,
		usage: n.usage,
		git: n.git,
		error: n.error,
		intent: n.intent ?? null,
		wait: n.wait ?? null,
		startedAt: n.started_at,
		finishedAt: n.finished_at
	}))
});

const withModes = (modes: string | null) => (modes === null ? {} : { modes_toml: modes });

export function createTracksClient(
	{ url, token }: TracksConfig,
	doFetch: typeof fetch = fetch
): TracksClient {
	async function call<T>(path: string, body?: unknown, timeoutMs = TIMEOUT_MS): Promise<T> {
		let res: Response;
		try {
			res = await doFetch(`${url}${path}`, {
				method: body === undefined ? 'GET' : 'POST',
				headers: {
					Authorization: `Bearer ${token}`,
					Accept: 'application/json',
					...(body === undefined ? {} : { 'Content-Type': 'application/json' })
				},
				body: body === undefined ? undefined : JSON.stringify(body),
				redirect: 'error',
				signal: AbortSignal.timeout(timeoutMs)
			});
		} catch {
			throw new TracksError(503, 'unavailable', 'enzarb-tracks could not be reached');
		}
		let json: unknown;
		try {
			json = await res.json();
		} catch {
			json = undefined;
		}
		if (res.ok && json !== undefined) return json as T;
		const err = (json as { error?: { code?: unknown; message?: unknown } } | undefined)?.error;
		const issues = (json as { issues?: unknown } | undefined)?.issues;
		throw new TracksError(
			res.status,
			typeof err?.code === 'string' ? err.code : 'unavailable',
			typeof err?.message === 'string' ? err.message : `enzarb-tracks returned ${res.status}`,
			Array.isArray(issues) ? (issues as ModeIssue[]) : []
		);
	}

	return {
		modesCatalog: () => call('/v1/modes/catalog'),
		parseModes: (text) => call('/v1/modes/parse', { text }),
		validateModes: async (doc) =>
			(await call<{ issues: ModeIssue[] }>('/v1/modes/validate', { doc })).issues,
		renderModes: (doc, old) => call('/v1/modes/render', old === null ? { doc } : { doc, old }),
		checkWorkflows: async (text, modes) => {
			const r = await call<{ report: { diagnostics: Diagnostic[] }; file?: WorkflowsFileWire }>(
				'/v1/workflows/check',
				{ workflows_toml: text, ...withModes(modes) }
			);
			return { diagnostics: r.report.diagnostics, file: r.file ?? null };
		},
		renderWorkflows: async (file, modes) => {
			const r = await call<{ text: string; report: { diagnostics: Diagnostic[] } }>(
				'/v1/workflows/render',
				{ file, ...withModes(modes) }
			);
			return { text: r.text, diagnostics: r.report.diagnostics };
		},
		decisions: async (eventIds) => {
			if (eventIds.length === 0) return [];
			const r = await call<{ decisions: WireDecision[] }>(
				`/v1/decisions?event_ids=${eventIds.join(',')}`
			);
			return r.decisions.map((d) => ({
				eventId: d.event_id,
				outcome: d.outcome,
				project: d.project,
				sha: d.sha,
				started: d.matched,
				reasons: d.reasons.map((x) => ({
					workflow: x.workflow,
					matched: x.matched,
					on: x.on ?? null,
					reason: x.reason
				})),
				errors: d.errors,
				resumed: d.resumed ?? [],
				decidedAt: d.decided_at
			}));
		},
		runs: async ({ project, eventIds }) => {
			const q = new URLSearchParams({ project });
			if (eventIds) q.set('event_ids', eventIds.join(','));
			if (eventIds && eventIds.length === 0) return [];
			const r = await call<{ runs: WireRun[] }>(`/v1/runs?${q}`);
			return r.runs.map(toRun);
		},
		run: async (id) => {
			try {
				return toDetail((await call<{ run: WireRun }>(`/v1/runs/${encodeURIComponent(id)}`)).run);
			} catch (e) {
				if (e instanceof TracksError && e.status === 404) return null;
				throw e;
			}
		},
		stopRun: async (id) => {
			try {
				await call(`/v1/runs/${encodeURIComponent(id)}/stop`, {});
				return true;
			} catch (e) {
				if (e instanceof TracksError && e.status === 409) return false;
				throw e;
			}
		},
		startRun: async (input) => {
			try {
				const r = await call<{ run: { id: string } }>('/v1/runs', input);
				return { id: r.run.id, alreadyRunning: false };
			} catch (e) {
				if (e instanceof TracksError && e.status === 409) return { id: null, alreadyRunning: true };
				throw e;
			}
		},
		resumeRun: async ({ id, actor, action, text }) => {
			try {
				await call(`/v1/runs/${encodeURIComponent(id)}/resume`, {
					actor,
					action,
					...(text ? { text } : {})
				});
				return { result: 'ok' };
			} catch (e) {
				if (e instanceof TracksError && e.status === 409) return { result: 'not_waiting' };
				if (e instanceof TracksError && e.status === 422 && e.code === 'not_accepted')
					return { result: 'not_accepted', message: e.message };
				throw e;
			}
		},
		dryRun: ({ text, modes, events, options }) =>
			call(
				'/v1/dry-run',
				{ workflows_toml: text, ...withModes(modes), events, ...(options ? { options } : {}) },
				DRY_RUN_TIMEOUT_MS
			)
	};
}

let shared: TracksClient | null | undefined;

/** The process's client, or null when tracks is not configured (the designers are off). */
export function getTracks(): TracksClient | null {
	if (shared === undefined) {
		const cfg = getConfig().tracks;
		shared = cfg ? createTracksClient(cfg) : null;
	}
	return shared;
}

/** Test hook. */
export function resetTracksSingleton(): void {
	shared = undefined;
}
