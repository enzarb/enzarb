import { getConfig, type TracksConfig } from './config.ts';
import type { ModeIssue, ModesCatalog, ModesDoc } from '#lib/modes-types.ts';

/**
 * Client for enzarb-tracks (cluster-internal, ADR-0029; API in tracks/README.md). The UI calls it
 * only after its own access checks. Every rule about modes lives in tracks (ez-core): this file
 * never re-derives a validation.
 */

const TIMEOUT_MS = 10_000;

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
}

export function createTracksClient(
	{ url, token }: TracksConfig,
	doFetch: typeof fetch = fetch
): TracksClient {
	async function call<T>(path: string, body?: unknown): Promise<T> {
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
				signal: AbortSignal.timeout(TIMEOUT_MS)
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
		renderModes: (doc, old) => call('/v1/modes/render', old === null ? { doc } : { doc, old })
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
