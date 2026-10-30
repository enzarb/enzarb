import { getConfig, type TracksConfig } from './config.ts';
import type { EventDecision } from '#lib/events.ts';
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
	decided_at: string;
}

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
				decidedAt: d.decided_at
			}));
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
