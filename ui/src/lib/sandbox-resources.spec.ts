import * as v from 'valibot';
import { describe, expect, it } from 'vitest';
import {
	CpuSchema,
	MemorySchema,
	SandboxResourcesSchema,
	StorageSchema,
	buildSandboxPatch,
	parseCpuInput,
	parseQuantity,
	parseQuantityInput,
	validateStorageGrowth
} from './sandbox-resources.ts';

const ok = (schema: v.GenericSchema, x: unknown) => v.safeParse(schema, x).success;

describe('CpuSchema', () => {
	for (const good of [1, 8, 64])
		it(`accepts ${good}`, () => expect(ok(CpuSchema, good)).toBe(true));
	for (const bad of [0, 65, 2.5, NaN, -1, '4', Infinity])
		it(`rejects ${bad}`, () => expect(ok(CpuSchema, bad)).toBe(false));
});

describe('MemorySchema', () => {
	for (const good of ['512Mi', '1024Mi', '8Gi', '256Gi', '262144Mi'])
		it(`accepts ${good}`, () => expect(ok(MemorySchema, good)).toBe(true));
	for (const bad of [
		'511Mi',
		'257Gi',
		'262145Mi',
		'0Gi',
		'08Gi',
		'4gi',
		'4G',
		'4',
		'',
		'1.5Gi',
		'1234567890Mi',
		4
	])
		it(`rejects ${JSON.stringify(bad)}`, () => expect(ok(MemorySchema, bad)).toBe(false));
	it('gives a floor message for 511Mi', () => {
		const r = v.safeParse(MemorySchema, '511Mi');
		expect(r.issues?.[0].message).toBe('Must be at least 512Mi');
	});
});

describe('quantities', () => {
	it('converts Mi and Gi to Mi', () => {
		expect(parseQuantity('1024Mi')).toBe(parseQuantity('1Gi'));
		expect(parseQuantity('20Gi')).toBe(20480);
		expect(parseQuantity('20gi')).toBeNull();
	});
});

describe('parseQuantityInput', () => {
	const valid: [string, string][] = [
		['16Gi', '16Gi'],
		['512Mi', '512Mi'],
		['1024Mi', '1Gi'],
		['1048576k', '1000Mi'],
		['2Ti', '2048Gi'],
		['1024Ki', '1Mi'],
		['  8Gi  ', '8Gi'],
		['1048576', '1Mi'],
		['1.5Gi', '1536Mi'],
		['0.5Gi', '512Mi']
	];
	for (const [text, normalized] of valid)
		it(`accepts ${JSON.stringify(text)} as ${normalized}`, () => {
			const r = parseQuantityInput(text);
			expect(r.ok && r.normalized).toBe(normalized);
		});
	for (const bad of [
		'',
		'abc',
		'1.5Mi',
		'1500Ki',
		'8G',
		'500m',
		'-1Gi',
		'1e3',
		'8GB',
		'0Gi',
		'100',
		'1E',
		'2P'
	])
		it(`rejects ${JSON.stringify(bad)}`, () => {
			const r = parseQuantityInput(bad);
			expect(r.ok).toBe(false);
			if (!r.ok) expect(r.message).not.toBe('');
		});
	it('rejects a result that is too long', () => {
		expect(parseQuantityInput('99999999999999Mi').ok).toBe(false);
	});
	it('leaves limits to the schemas, and the strict schema rejects 8G', () => {
		const q = parseQuantityInput('300Gi');
		expect(q.ok && ok(MemorySchema, q.normalized)).toBe(false);
		expect(ok(MemorySchema, '8G')).toBe(false);
	});
});

describe('parseCpuInput', () => {
	it('accepts whole cores and empty', () => {
		expect(parseCpuInput('8')).toEqual({ ok: true, value: 8 });
		expect(parseCpuInput(' ')).toEqual({ ok: true, value: null });
	});
	for (const bad of ['500m', '0.5', '0', '65', 'x', '-1'])
		it(`rejects ${bad}`, () => expect(parseCpuInput(bad).ok).toBe(false));
});

describe('storage', () => {
	it('caps at 256Gi', () => {
		expect(ok(StorageSchema, '256Gi')).toBe(true);
		expect(ok(StorageSchema, '257Gi')).toBe(false);
		expect(ok(StorageSchema, '20')).toBe(false);
	});
	it('only grows, treating unset as 20Gi', () => {
		expect(validateStorageGrowth('40Gi', '30Gi')).toMatch(/only grow/);
		expect(validateStorageGrowth('40Gi', '40Gi')).toBeNull();
		expect(validateStorageGrowth('40Gi', '41Gi')).toBeNull();
		expect(validateStorageGrowth(null, '10Gi')).toMatch(/20Gi/);
		expect(validateStorageGrowth(undefined, '20Gi')).toBeNull();
		expect(validateStorageGrowth('1Gi', '1025Mi')).toBeNull();
		expect(validateStorageGrowth('2Gi', '1025Mi')).not.toBeNull();
	});
});

describe('SandboxResourcesSchema / buildSandboxPatch', () => {
	it('accepts nulls and an omitted storage', () => {
		expect(ok(SandboxResourcesSchema, { cpu: null, memory: null })).toBe(true);
		expect(ok(SandboxResourcesSchema, { cpu: 2.5, memory: null })).toBe(false);
		expect(ok(SandboxResourcesSchema, { memory: null })).toBe(false);
	});
	it('sends null for cleared fields and omits unset storage', () => {
		expect(buildSandboxPatch({ cpu: null, memory: null })).toEqual({
			sandbox: { cpu: null, memory: null }
		});
		expect(buildSandboxPatch({ cpu: 8, memory: '16Gi', storage: '40Gi' })).toEqual({
			sandbox: { cpu: 8, memory: '16Gi', storage: '40Gi' }
		});
	});
});
