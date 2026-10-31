import { describe, expect, it } from 'vitest';
import { MAX_PATCH_CHARS, notInline, parsePatch } from './patch.ts';

describe('parsePatch', () => {
	it('numbers added, removed and context lines across hunks', () => {
		const lines = parsePatch(
			['@@ -1,3 +1,3 @@', ' a', '-b', '+B', ' c', '@@ -10,2 +10,3 @@', ' x', '+y', ' z'].join('\n')
		);
		expect(lines.map((l) => [l.kind, l.text, l.oldNo, l.newNo])).toEqual([
			['hunk', '@@ -1,3 +1,3 @@', null, null],
			['ctx', 'a', 1, 1],
			['del', 'b', 2, null],
			['add', 'B', null, 2],
			['ctx', 'c', 3, 3],
			['hunk', '@@ -10,2 +10,3 @@', null, null],
			['ctx', 'x', 10, 10],
			['add', 'y', null, 11],
			['ctx', 'z', 11, 12]
		]);
	});

	it('keeps "No newline" markers as notes and ignores a trailing newline', () => {
		const lines = parsePatch('@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b\n');
		expect(lines.map((l) => l.kind)).toEqual(['hunk', 'del', 'note', 'add']);
		expect(lines[2]!.text).toBe('No newline at end of file');
	});

	it('treats code that starts with +++ or --- inside a hunk as changes', () => {
		const lines = parsePatch('@@ -1 +1 @@\n---x\n+++y');
		expect(lines.map((l) => l.kind)).toEqual(['hunk', 'del', 'add']);
	});
});

describe('notInline', () => {
	const f = { filename: 'f', status: 'modified', additions: 1, deletions: 0 };
	it('shows normal patches', () =>
		expect(notInline({ ...f, patch: '@@ -1 +1 @@\n+a' })).toBeNull());
	it('refuses binary or oversized ones', () => {
		expect(notInline(f)).toBe('too large to show inline');
		expect(notInline({ ...f, patch: 'x'.repeat(MAX_PATCH_CHARS + 1) })).toBe(
			'too large to show inline'
		);
		expect(notInline({ ...f, additions: 0 })).toBe('no content changes');
	});
});
