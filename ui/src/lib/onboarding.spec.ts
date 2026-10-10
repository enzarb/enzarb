import { describe, expect, it } from 'vitest';
import {
	expandNameTemplate,
	planProblems,
	slugifyName,
	splitRepoKey,
	validateProjectName
} from './onboarding.ts';

describe('slugifyName', () => {
	it.each([
		['Enzarb-UI', 'enzarb-ui'],
		['my_repo.name', 'my-repo-name'],
		['--lead and trail--', 'lead-and-trail'],
		['a   b', 'a-b'],
		['Café Münch', 'cafe-munch'],
		['日本語', ''],
		['', ''],
		['---', ''],
		['9lives', '9lives']
	])('%j -> %j', (input, out) => expect(slugifyName(input)).toBe(out));

	it('truncates to 50 characters without leaving a trailing hyphen', () => {
		const long = slugifyName(`${'a'.repeat(49)}-bbbbbb`);
		expect(long).toBe('a'.repeat(49));
		expect(slugifyName('x'.repeat(80))).toHaveLength(50);
	});

	it('always yields a valid name or the empty string', () => {
		for (const s of ['A.B', '_x_', 'x'.repeat(60), 'a--b', '.github', 'repo.git'])
			expect(validateProjectName(slugifyName(s))).toBeUndefined();
	});
});

describe('expandNameTemplate / splitRepoKey', () => {
	const parts = { owner: 'Acme', repo: 'Web.App' };
	it('expands placeholders then slugifies', () => {
		expect(expandNameTemplate('{repo}', parts)).toBe('web-app');
		expect(expandNameTemplate('{owner}-{repo}', parts)).toBe('acme-web-app');
		expect(expandNameTemplate('proj {repo} v2', parts)).toBe('proj-web-app-v2');
	});
	it('splits keys', () => {
		expect(splitRepoKey('github:a/b')).toEqual({ owner: 'a', repo: 'b' });
		expect(splitRepoKey('gitlab:a/b')).toBeUndefined();
		expect(splitRepoKey('github:a')).toBeUndefined();
	});
});

describe('validateProjectName', () => {
	it('accepts DNS labels up to 50 characters', () => {
		expect(validateProjectName('a')).toBeUndefined();
		expect(validateProjectName('a'.repeat(50))).toBeUndefined();
		expect(validateProjectName('a-1')).toBeUndefined();
	});
	it.each(['', ' ', 'a'.repeat(51), '-a', 'a-', 'A', 'a_b', 'a.b'])('rejects %j', (n) =>
		expect(validateProjectName(n)).toBeTruthy()
	);
});

describe('planProblems', () => {
	const plan = (name: string, repositories = ['github:a/b']) => ({ name, repositories });
	it('is empty for good plans', () => {
		expect(planProblems([plan('x'), plan('y')], new Set())).toEqual([[], []]);
	});
	it('flags more than 50 projects per request', () => {
		const many = Array.from({ length: 51 }, (_, i) => plan(`p${i}`));
		expect(planProblems(many, new Set())[0]).toEqual(['At most 50 projects per request']);
		expect(planProblems(many.slice(0, 50), new Set()).flat()).toEqual([]);
	});
	it('flags invalid, existing and repeated names, and empty repository lists', () => {
		const out = planProblems(
			[plan('Bad'), plan('taken'), plan('dup'), plan('dup'), plan('ok', [])],
			new Set(['taken'])
		);
		expect(out[0][0]).toMatch(/lowercase/i);
		expect(out[1][0]).toMatch(/already exists/);
		expect(out[2][0]).toMatch(/more than once/);
		expect(out[3][0]).toMatch(/more than once/);
		expect(out[4][0]).toMatch(/at least one/i);
	});
});
