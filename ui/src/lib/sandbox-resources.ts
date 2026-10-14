import * as v from 'valibot';

/** Operator default for `spec.sandbox.storage` when unset (operator v0.8.0). */
export const DEFAULT_STORAGE = '20Gi';

export const MIN_CPU = 1;
export const MAX_CPU = 64;
export const MIN_MEMORY_MI = 512;
export const MAX_MEMORY_MI = 256 * 1024;
/** UI-only cap on storage; the CRD itself has no storage limit. */
export const MAX_STORAGE_MI = 256 * 1024;
/** Mirrors the CRD's `maxLength` for `spec.sandbox.memory`. */
export const MAX_QUANTITY_LENGTH = 12;

/** CRD grammar for `spec.sandbox.memory`; storage uses the same one. */
const QUANTITY_RE = /^[1-9][0-9]*(Mi|Gi)$/;

/** Size in Mi of a `<n>Mi` / `<n>Gi` string, or null when it does not match the grammar. */
export function parseQuantity(s: string): number | null {
	if (s.length > MAX_QUANTITY_LENGTH || !QUANTITY_RE.test(s)) return null;
	const n = Number(s.slice(0, -2));
	return s.endsWith('Gi') ? n * 1024 : n;
}

export const CpuSchema = v.pipe(
	v.number('Enter a whole number'),
	v.integer('Enter a whole number'),
	v.minValue(MIN_CPU, `Must be at least ${MIN_CPU}`),
	v.maxValue(MAX_CPU, `Must be at most ${MAX_CPU}`)
);

const quantity = (min: number, max: number, maxLabel: string) =>
	v.pipe(
		v.string('Enter a size such as 8Gi'),
		v.check(
			(s) => s.length <= MAX_QUANTITY_LENGTH && QUANTITY_RE.test(s),
			'Use a whole number followed by Mi or Gi, e.g. 8Gi'
		),
		v.check((s) => {
			const mi = parseQuantity(s);
			return mi === null || mi >= min;
		}, `Must be at least ${min}Mi`),
		v.check((s) => {
			const mi = parseQuantity(s);
			return mi === null || mi <= max;
		}, `Must be at most ${maxLabel}`)
	);

export const MemorySchema = quantity(MIN_MEMORY_MI, MAX_MEMORY_MI, '256Gi');
export const StorageSchema = quantity(1, MAX_STORAGE_MI, '256Gi');

/** Storage may only grow. `current` defaults to the operator default when unset. Returns an error or null. */
export function validateStorageGrowth(
	current: string | null | undefined,
	next: string
): string | null {
	const cur = parseQuantity(current || DEFAULT_STORAGE);
	const nxt = parseQuantity(next);
	if (cur === null || nxt === null) return null; // the schema reports malformed values
	return nxt < cur ? `Storage can only grow (currently ${current || DEFAULT_STORAGE})` : null;
}

/** `cpu`/`memory` null clears the limit; `storage` null/omitted leaves it untouched. */
export const SandboxResourcesSchema = v.object({
	cpu: v.nullable(CpuSchema),
	memory: v.nullable(MemorySchema),
	storage: v.optional(v.nullable(StorageSchema))
});
export type SandboxResourcesInput = v.InferOutput<typeof SandboxResourcesSchema>;

/**
 * Merge-patch body for `spec`: only `sandbox.{cpu,memory,storage}`. A cleared cpu/memory is sent
 * as `null`, which removes the key; an unset storage is left out so it is never shrunk or reset.
 * `spec.sandbox.resources` is never written.
 */
export function buildSandboxPatch(input: SandboxResourcesInput): {
	sandbox: { cpu: number | null; memory: string | null; storage?: string };
} {
	return {
		sandbox: {
			cpu: input.cpu,
			memory: input.memory,
			...(input.storage ? { storage: input.storage } : {})
		}
	};
}

const INPUT_RE = /^([0-9]+)(?:\.([0-9]+))?(Ki|Mi|Gi|Ti|k|M|G|T)?$/;
const MI = 1024n * 1024n;
const UNIT_BYTES: Record<string, bigint> = {
	'': 1n,
	Ki: 1024n,
	Mi: MI,
	Gi: 1024n * MI,
	Ti: 1024n * 1024n * MI,
	k: 1000n,
	M: 1000n ** 2n,
	G: 1000n ** 3n,
	T: 1000n ** 4n
};
const WHOLE_MI = 'Must be a whole number of Mi, e.g. 512Mi or 8Gi';
const TOO_LARGE = `Too large; at most ${MAX_STORAGE_MI / 1024}Gi`;

export type QuantityInput =
	{ ok: true; mi: number; normalized: string } | { ok: false; message: string };

/** Largest exact unit the CRD grammar allows: `1024Mi` becomes `1Gi`, `1500Mi` stays. */
export function normalizeQuantity(mi: number): string {
	return mi % 1024 === 0 ? `${mi / 1024}Gi` : `${mi}Mi`;
}

/**
 * Parse a typed Kubernetes quantity (binary and decimal suffixes) into the CRD's `<n>Mi|Gi`
 * form. Exact integer maths; anything that is not a whole number of Mi is rejected.
 */
export function parseQuantityInput(text: string): QuantityInput {
	const t = text.trim();
	const m = INPUT_RE.exec(t);
	if (!m) {
		if (t === '') return { ok: false, message: 'Enter a size such as 8Gi' };
		if (t.startsWith('-')) return { ok: false, message: 'Must be greater than zero' };
		if (/^[0-9.]+m$/.test(t))
			return { ok: false, message: 'Milli units are not supported; use Mi or Gi, e.g. 512Mi' };
		return {
			ok: false,
			message: 'Use a number with a Ki, Mi, Gi, Ti, k, M, G or T suffix, e.g. 8Gi'
		};
	}
	const [, int, frac = '', unit = ''] = m;
	const scale = 10n ** BigInt(frac.length);
	const scaled = BigInt(int + frac) * UNIT_BYTES[unit];
	if (scaled % scale !== 0n) return { ok: false, message: WHOLE_MI };
	const bytes = scaled / scale;
	if (bytes === 0n) return { ok: false, message: 'Must be greater than zero' };
	if (bytes % MI !== 0n) return { ok: false, message: WHOLE_MI };
	const big = bytes / MI;
	if (big > BigInt(Number.MAX_SAFE_INTEGER)) return { ok: false, message: TOO_LARGE };
	const mi = Number(big);
	const normalized = normalizeQuantity(mi);
	if (normalized.length > MAX_QUANTITY_LENGTH) return { ok: false, message: TOO_LARGE };
	return { ok: true, mi, normalized };
}

export type CpuInput = { ok: true; value: number | null } | { ok: false; message: string };

/** Parse typed whole cores; empty means no limit. Fractions need a CRD change, so they are rejected. */
export function parseCpuInput(text: string): CpuInput {
	const t = text.trim();
	if (t === '') return { ok: true, value: null };
	if (/^[0-9]*\.[0-9]+$/.test(t) || /^[0-9]+m$/.test(t))
		return {
			ok: false,
			message: `Fractions of a core are not supported yet; use whole cores (${MIN_CPU}-${MAX_CPU})`
		};
	if (!/^[0-9]+$/.test(t)) return { ok: false, message: 'Enter a whole number' };
	const r = v.safeParse(CpuSchema, Number(t));
	return r.success ? { ok: true, value: r.output } : { ok: false, message: r.issues[0].message };
}
