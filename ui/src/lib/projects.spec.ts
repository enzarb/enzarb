import * as v from 'valibot';
import { describe, expect, it } from 'vitest';
import { CreateProjectSchema, buildProject, repoKey, toView } from './projects.ts';

const parse = (x: unknown) => v.safeParse(CreateProjectSchema, x);
const ok = (x: unknown) => v.parse(CreateProjectSchema, x);

describe('repoKey', () => {
	it('lowercases canonical github urls', () => {
		expect(repoKey('https://github.com/EnZarb/enzarb-UI')).toBe('github:enzarb/enzarb-ui');
	});
	it.each([
		'http://github.com/a/b',
		'https://github.com/a/b/',
		'https://github.com/a/b.git',
		'https://github.com/a',
		'https://gitlab.com/a/b',
		'https://github.com/a/b/tree/main',
		'git@github.com:a/b'
	])('rejects %s', (u) => expect(repoKey(u)).toBeUndefined());
});

describe('CreateProjectSchema', () => {
	it('accepts a minimal project and applies defaults', () => {
		const r = parse({ name: 'enzarb' });
		expect(r.success).toBe(true);
		expect(ok({ name: 'enzarb' })).toMatchObject({
			displayName: '',
			repositories: [],
			issues: false
		});
	});
	it.each(['', 'Upper', '-a', 'a-', 'a_b', 'a.b', 'x'.repeat(55)])('rejects name %j', (name) => {
		expect(parse({ name }).success).toBe(false);
	});
	it('accepts a 54 character name', () => {
		expect(parse({ name: 'a'.repeat(54) }).success).toBe(true);
	});
	it('rejects non-canonical repository urls', () => {
		const r = parse({ name: 'p', repositories: [{ url: 'https://github.com/a/b.git' }] });
		expect(r.success).toBe(false);
	});
	it('defaults the branch to main and rejects duplicates by repo key', () => {
		const good = ok({ name: 'p', repositories: [{ url: 'https://github.com/a/b' }] });
		expect(good.repositories[0].defaultBranch).toBe('main');
		const dup = parse({
			name: 'p',
			repositories: [{ url: 'https://github.com/a/b' }, { url: 'https://github.com/A/B' }]
		});
		expect(dup.success).toBe(false);
	});
});

describe('docker options', () => {
	it('defaults to no docker and no sandbox block', () => {
		expect(ok({ name: 'x' })).toMatchObject({ docker: false, runtimeClassName: '' });
		expect(buildProject(ok({ name: 'x' }), 'u').spec.sandbox).toBeUndefined();
	});
	it('requires a runtime class when docker is on', () => {
		const r = parse({ name: 'x', docker: true });
		expect(r.success).toBe(false);
		expect(r.issues?.[0].message).toMatch(/runtime class/i);
		expect(r.issues?.[0].path?.[0].key).toBe('runtimeClassName');
		expect(parse({ name: 'x', docker: true, runtimeClassName: ' ' }).success).toBe(false);
	});
	it('writes sandbox.docker and runtimeClassName into the CR', () => {
		const p = buildProject(ok({ name: 'x', docker: true, runtimeClassName: 'gvisor' }), 'u');
		expect(p.spec.sandbox).toEqual({ runtimeClassName: 'gvisor', docker: { enabled: true } });
		expect(toView(p)).toMatchObject({ dockerEnabled: true, runtimeClassName: 'gvisor' });
	});
});

describe('buildProject / toView', () => {
	it('writes a contracts §2 Project owned by the login', () => {
		const input = ok({
			name: 'enzarb',
			displayName: 'Enzarb',
			repositories: [{ url: 'https://github.com/enzarb/enzarb-ui', defaultBranch: 'dev' }],
			issues: true,
			mentions: true
		});
		expect(buildProject(input, 'ruckc')).toEqual({
			apiVersion: 'enzarb.dev/v1alpha1',
			kind: 'Project',
			metadata: { name: 'enzarb' },
			spec: {
				owner: 'ruckc',
				displayName: 'Enzarb',
				repositories: [
					{ url: 'https://github.com/enzarb/enzarb-ui', connector: 'github', defaultBranch: 'dev' }
				],
				runMode: 'worktree',
				triggers: { issues: true, changeRequests: false, mentions: true }
			}
		});
	});
	it('falls back to the name and reports readiness', () => {
		const p = buildProject(ok({ name: 'x' }), 'u');
		p.spec.displayName = '';
		expect(toView(p)).toMatchObject({ displayName: 'x', ready: false, namespace: null });
		p.status = { conditions: [{ type: 'Ready', status: 'True' }], namespace: 'enzarb-p-x' };
		expect(toView(p)).toMatchObject({ ready: true, namespace: 'enzarb-p-x' });
	});
});
