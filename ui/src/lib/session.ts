import type { SessionEvent } from '#lib/tasks.ts';

/**
 * Display helpers for captured session events (`session_events.payload`). Payloads are free-form
 * per runtime; these read the common fields and fall back to JSON.
 */

type P = Record<string, unknown>;
const obj = (x: unknown): P => (x && typeof x === 'object' && !Array.isArray(x) ? (x as P) : {});

/** The text of a message-like event, or null. */
export function eventText(e: SessionEvent): string | null {
	if (typeof e.payload === 'string') return e.payload;
	const p = obj(e.payload);
	for (const k of ['text', 'content', 'body', 'message', 'line']) {
		if (typeof p[k] === 'string') return p[k] as string;
	}
	return null;
}

export function toolName(e: SessionEvent): string {
	const p = obj(e.payload);
	return String(p.name ?? p.tool ?? p.title ?? 'tool');
}

/** The tool call input / result output as text. */
export function toolBody(e: SessionEvent): string {
	const p = obj(e.payload);
	const v = e.kind === 'tool_call' ? (p.input ?? p.args ?? p.arguments) : (p.output ?? p.content);
	if (typeof v === 'string') return v;
	return JSON.stringify(v ?? e.payload, null, 2);
}

export function isError(e: SessionEvent): boolean {
	const p = obj(e.payload);
	return (
		e.kind === 'stderr' ||
		p.is_error === true ||
		p.isError === true ||
		p.error !== undefined ||
		p.level === 'error' ||
		p.status === 'failed'
	);
}

export interface UsageFigures {
	input: number | null;
	output: number | null;
	cost: number | null;
}

export function usageOf(e: SessionEvent): UsageFigures {
	const p = obj(e.payload);
	const n = (x: unknown) => (typeof x === 'number' && Number.isFinite(x) ? x : null);
	return {
		input: n(p.input_tokens ?? p.input),
		output: n(p.output_tokens ?? p.output),
		cost: n(p.cost_usd ?? p.cost)
	};
}

/** Case-insensitive search over the raw payload. */
export function matches(e: SessionEvent, query: string): boolean {
	const q = query.trim().toLowerCase();
	if (!q) return true;
	return (
		e.kind.includes(q) ||
		(typeof e.payload === 'string' ? e.payload : JSON.stringify(e.payload))
			.toLowerCase()
			.includes(q)
	);
}

/** Append a page, dropping sequence numbers already held. */
export function appendEvents(have: readonly SessionEvent[], page: readonly SessionEvent[]) {
	const last = have.length ? have[have.length - 1].seq : -Infinity;
	return [...have, ...page.filter((e) => e.seq > last)];
}
