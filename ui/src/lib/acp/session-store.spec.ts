import { afterEach, describe, expect, it, vi } from 'vitest';
import { clear, load, save } from './session-store.ts';

function fakeStorage(): Storage {
	const m = new Map<string, string>();
	return {
		getItem: (k: string) => m.get(k) ?? null,
		setItem: (k: string, v: string) => void m.set(k, v),
		removeItem: (k: string) => void m.delete(k),
		clear: () => m.clear(),
		key: () => null,
		get length() {
			return m.size;
		}
	} as Storage;
}

afterEach(() => vi.unstubAllGlobals());

describe('session-store', () => {
	it('round-trips per project and clears', () => {
		const s = fakeStorage();
		vi.stubGlobal('sessionStorage', s);
		save('a', 'sess-1');
		save('b', 'sess-2');
		expect(s.getItem('enzarb.chat.a')).toBe('sess-1');
		expect(load('a')).toBe('sess-1');
		clear('a');
		expect(load('a')).toBeUndefined();
		expect(load('b')).toBe('sess-2');
	});

	it('degrades to no resume without storage', () => {
		vi.stubGlobal('sessionStorage', undefined);
		expect(() => save('a', 'x')).not.toThrow();
		expect(load('a')).toBeUndefined();
		expect(() => clear('a')).not.toThrow();
	});

	it('swallows storage errors', () => {
		const boom = () => {
			throw new Error('denied');
		};
		vi.stubGlobal('sessionStorage', { getItem: boom, setItem: boom, removeItem: boom });
		expect(load('a')).toBeUndefined();
		expect(() => save('a', 'x')).not.toThrow();
		expect(() => clear('a')).not.toThrow();
	});

	const invalid: [string, string][] = [
		['over-long', 'a'.repeat(129)],
		['empty', ''],
		['space', 'sess 1'],
		['slash', 'sess/1'],
		['control char', 'sess\u0000\n1'],
		['unicode', 'sess-é-1']
	];

	it.each(invalid)('load() rejects and clears a %s value', (_n, bad) => {
		const s = fakeStorage();
		vi.stubGlobal('sessionStorage', s);
		s.setItem('enzarb.chat.a', bad);
		expect(load('a')).toBeUndefined();
		expect(s.getItem('enzarb.chat.a')).toBeNull();
	});

	it.each(invalid)('save() ignores a %s id', (_n, bad) => {
		const s = fakeStorage();
		vi.stubGlobal('sessionStorage', s);
		save('a', bad);
		expect(s.getItem('enzarb.chat.a')).toBeNull();
	});

	it('accepts a 128-char id and typical ids', () => {
		vi.stubGlobal('sessionStorage', fakeStorage());
		for (const id of ['a'.repeat(128), 'sess-1', 'a.b_c:d-e']) {
			save('a', id);
			expect(load('a')).toBe(id);
		}
	});
});
