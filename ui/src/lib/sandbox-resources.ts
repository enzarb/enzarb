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

export type QuantityUnit = 'Mi' | 'Gi';

/** CRD grammar for `spec.sandbox.memory`; storage uses the same one. */
const QUANTITY_RE = /^[1-9][0-9]*(Mi|Gi)$/;

/** Size in Mi of a `<n>Mi` / `<n>Gi` string, or null when it does not match the grammar. */
export function parseQuantity(s: string): number | null {
	if (s.length > MAX_QUANTITY_LENGTH || !QUANTITY_RE.test(s)) return null;
	const n = Number(s.slice(0, -2));
	return s.endsWith('Gi') ? n * 1024 : n;
}

export function toMi(value: number, unit: QuantityUnit): number {
	return unit === 'Gi' ? value * 1024 : value;
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

/** Split `8Gi` into a form value and unit; `512Mi` stays Mi. Null when it does not parse. */
export function splitQuantity(s: string | null | undefined): { value: string; unit: QuantityUnit } {
	const m = s ? /^([0-9]+)(Mi|Gi)$/.exec(s) : null;
	return m ? { value: m[1], unit: m[2] as QuantityUnit } : { value: '', unit: 'Gi' };
}

/** Join a form value and unit into a Quantity string; empty value gives null. */
export function joinQuantity(value: string | number | null, unit: QuantityUnit): string | null {
	const s = String(value ?? '').trim();
	return s === '' ? null : `${s}${unit}`;
}
