import type {
	PermissionChoice,
	PermissionOption,
	PermissionOptionKind,
	PermissionStatus
} from './types.ts';

const MAX_TEXT = 200;

function text(v: unknown): string | undefined {
	return typeof v === 'string' && v.length > 0 ? v.slice(0, MAX_TEXT) : undefined;
}

/**
 * Keep only `allow_once` / `reject_once` options (never render `allow_always` / `reject_always`).
 * `duplicate` is set when the request repeats an `optionId`: the sandbox cancels those, so the
 * caller answers `cancelled` and shows no buttons.
 */
export function sanitizeOptions(raw: unknown): { options: PermissionOption[]; duplicate: boolean } {
	const list = Array.isArray(raw) ? raw : [];
	const seen = new Set<string>();
	let duplicate = false;
	const options: PermissionOption[] = [];
	for (const o of list) {
		if (typeof o !== 'object' || o === null) continue;
		const r = o as Record<string, unknown>;
		const id = text(r.optionId);
		if (!id) continue;
		if (seen.has(id)) duplicate = true;
		seen.add(id);
		if (r.kind !== 'allow_once' && r.kind !== 'reject_once') continue;
		options.push({ optionId: id, name: text(r.name) ?? id, kind: r.kind });
	}
	return { options, duplicate };
}

/** `_meta.enzarb = {policy, rule, reason}` as display text; undefined when absent or malformed. */
export function policyHint(meta: unknown): string | undefined {
	if (typeof meta !== 'object' || meta === null) return undefined;
	const e = (meta as Record<string, unknown>).enzarb;
	if (typeof e !== 'object' || e === null) return undefined;
	const { policy, rule } = e as Record<string, unknown>;
	const p = text(policy);
	const r = text(rule);
	if (!p || !r) return undefined;
	return `Sandbox policy: ${p === 'deny' ? 'would deny' : p} — ${r}`;
}

const KIND_FOR: Record<PermissionChoice, PermissionOptionKind> = {
	allow: 'allow_once',
	deny: 'reject_once'
};

/** The option id of the wanted kind from this same request, if it offered one. */
export function optionFor(
	options: readonly PermissionOption[],
	choice: PermissionChoice
): string | undefined {
	return options.find((o) => o.kind === KIND_FOR[choice])?.optionId;
}

export type PermissionOutcome =
	{ outcome: 'selected'; optionId: string } | { outcome: 'cancelled' };

interface Entry {
	epoch: number;
	options: readonly PermissionOption[];
	settle: (o: PermissionOutcome) => void;
}

/** Pending permission requests, each tagged with the connection epoch it arrived on. */
export class PermissionRegistry {
	private entries = new Map<string, Entry>();

	register(
		id: string,
		epoch: number,
		options: readonly PermissionOption[],
		settle: (o: PermissionOutcome) => void
	): void {
		this.entries.set(id, { epoch, options, settle });
	}

	has(id: string): boolean {
		return this.entries.has(id);
	}

	/**
	 * Answer a request. A stale epoch, an unknown or already-answered id, or a choice the request
	 * did not offer does nothing and returns null.
	 */
	answer(id: string, choice: PermissionChoice, epoch: number): PermissionStatus | null {
		const e = this.entries.get(id);
		if (!e || e.epoch !== epoch) return null;
		const optionId = optionFor(e.options, choice);
		if (!optionId) return null;
		this.entries.delete(id);
		e.settle({ outcome: 'selected', optionId });
		return choice === 'allow' ? 'allowed' : 'denied';
	}

	/** Settle one request as cancelled. */
	cancel(id: string): boolean {
		const e = this.entries.get(id);
		if (!e) return false;
		this.entries.delete(id);
		e.settle({ outcome: 'cancelled' });
		return true;
	}

	/** Settle every pending request as cancelled; returns their ids. */
	cancelAll(): string[] {
		const ids = [...this.entries.keys()];
		for (const id of ids) this.cancel(id);
		return ids;
	}
}
