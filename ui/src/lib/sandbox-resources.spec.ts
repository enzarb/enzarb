import * as v from 'valibot';
import { describe, expect, it } from 'vitest';
import {
	CpuSchema,
	MemorySchema,
	SandboxResourcesSchema,
	StorageSchema,
	buildSandboxPatch,
	joinQuantity,
	parseQuantity,
	splitQuantity,
	toMi,
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
		expect(toMi(2, 'Gi')).toBe(2048);
		expect(toMi(2, 'Mi')).toBe(2);
	});
	it('splits and joins form values', () => {
		expect(splitQuantity('512Mi')).toEqual({ value: '512', unit: 'Mi' });
		expect(splitQuantity(null)).toEqual({ value: '', unit: 'Gi' });
		expect(joinQuantity('8', 'Gi')).toBe('8Gi');
		expect(joinQuantity('  ', 'Gi')).toBeNull();
	});
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
