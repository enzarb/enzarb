import { describe, expect, it, vi } from 'vitest';
import { PermissionRegistry, optionFor, policyHint, sanitizeOptions } from './permissions.ts';

const opts = [
	{ optionId: 'a', name: 'Allow', kind: 'allow_once' },
	{ optionId: 'aa', name: 'Always', kind: 'allow_always' },
	{ optionId: 'r', name: 'Reject', kind: 'reject_once' },
	{ optionId: 'ra', name: 'Never', kind: 'reject_always' }
];

describe('sanitizeOptions', () => {
	it('drops always-variants entirely', () => {
		const { options, duplicate } = sanitizeOptions(opts);
		expect(options.map((o) => o.optionId)).toEqual(['a', 'r']);
		expect(duplicate).toBe(false);
	});

	it('a request offering only allow_always has no Allow option', () => {
		const { options } = sanitizeOptions([opts[1]]);
		expect(optionFor(options, 'allow')).toBeUndefined();
	});

	it('flags duplicate option ids, even across dropped kinds', () => {
		expect(sanitizeOptions([opts[0], { ...opts[0] }]).duplicate).toBe(true);
		expect(
			sanitizeOptions([opts[0], { optionId: 'a', name: 'x', kind: 'allow_always' }]).duplicate
		).toBe(true);
	});

	it('tolerates junk', () => {
		expect(sanitizeOptions('x')).toEqual({ options: [], duplicate: false });
		expect(sanitizeOptions([null, 3, { kind: 'allow_once' }]).options).toEqual([]);
	});
});

describe('policyHint', () => {
	it('formats a deny hint', () => {
		expect(policyHint({ enzarb: { policy: 'deny', rule: 'r1', reason: 'x' } })).toBe(
			'Sandbox policy: would deny — r1'
		);
	});
	it.each([[undefined], [{}], [{ enzarb: 1 }], [{ enzarb: { policy: 'deny' } }]])(
		'ignores %j',
		(m) => {
			expect(policyHint(m)).toBeUndefined();
		}
	);
	it('keeps markup as plain text', () => {
		expect(policyHint({ enzarb: { policy: 'deny', rule: '<img onerror=x>' } })).toContain(
			'<img onerror=x>'
		);
	});
});

describe('PermissionRegistry', () => {
	const make = () => {
		const settle = vi.fn();
		const reg = new PermissionRegistry();
		reg.register('p', 1, sanitizeOptions(opts).options, settle);
		return { reg, settle };
	};

	it('sends the same-request option id for each choice', () => {
		const a = make();
		expect(a.reg.answer('p', 'allow', 1)).toBe('allowed');
		expect(a.settle).toHaveBeenCalledWith({ outcome: 'selected', optionId: 'a' });
		const d = make();
		expect(d.reg.answer('p', 'deny', 1)).toBe('denied');
		expect(d.settle).toHaveBeenCalledWith({ outcome: 'selected', optionId: 'r' });
	});

	it('a double click sends once', () => {
		const { reg, settle } = make();
		reg.answer('p', 'allow', 1);
		expect(reg.answer('p', 'allow', 1)).toBeNull();
		expect(settle).toHaveBeenCalledTimes(1);
	});

	it('a stale epoch does nothing', () => {
		const { reg, settle } = make();
		expect(reg.answer('p', 'allow', 0)).toBeNull();
		expect(settle).not.toHaveBeenCalled();
		expect(reg.has('p')).toBe(true);
	});

	it('a late click after cancelAll does nothing', () => {
		const { reg, settle } = make();
		expect(reg.cancelAll()).toEqual(['p']);
		expect(settle).toHaveBeenCalledWith({ outcome: 'cancelled' });
		expect(reg.answer('p', 'allow', 1)).toBeNull();
		expect(settle).toHaveBeenCalledTimes(1);
	});

	it('refuses a choice the request did not offer', () => {
		const settle = vi.fn();
		const reg = new PermissionRegistry();
		reg.register('p', 1, sanitizeOptions([opts[2]]).options, settle);
		expect(reg.answer('p', 'allow', 1)).toBeNull();
		expect(settle).not.toHaveBeenCalled();
	});
});
