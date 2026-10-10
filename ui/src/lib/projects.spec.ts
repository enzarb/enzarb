import { describe, expect, it } from 'vitest';
import { buildProject, normalizeRepoKey, toView, type NewProject } from './projects.ts';

const mk = (over: Partial<NewProject> = {}): NewProject => ({
	name: 'x',
	displayName: '',
	repositories: [],
	triggers: { issues: false, changeRequests: false, mentions: false },
	...over
});

describe('normalizeRepoKey (mirrors the operator RepoKey::from_connector_url)', () => {
	it.each([
		['https://github.com/EnZarb/enzarb-UI', 'github:enzarb/enzarb-ui'],
		['https://www.github.com/a/b', 'github:a/b'],
		['https://github.com/a/b/', 'github:a/b'],
		['https://github.com/a/b///', 'github:a/b'],
		['https://github.com/a/b.git', 'github:a/b'],
		['https://github.com/a/b.git/', 'github:a/b'],
		['  https://github.com/a/b  ', 'github:a/b'],
		['https://ghe.example.com/a/b', 'github:a/b'],
		['https://github.com/a_b/c.d-e', 'github:a_b/c.d-e']
	])('%s -> %s', (u, key) => expect(normalizeRepoKey('github', u)).toBe(key));

	it('uses the connector field, not the host', () => {
		expect(normalizeRepoKey('Gitea', 'https://github.com/a/b')).toBe('gitea:a/b');
	});
	it.each([
		'http://github.com/a/b',
		'https://github.com',
		'https://github.com/a',
		'https://github.com/a/b/tree/main',
		'https://github.com//b',
		'https://github.com/a b/c',
		'git@github.com:a/b',
		''
	])('rejects %j', (u) => expect(normalizeRepoKey('github', u)).toBeUndefined());
	it('rejects an empty or odd connector', () => {
		expect(normalizeRepoKey('', 'https://github.com/a/b')).toBeUndefined();
		expect(normalizeRepoKey('gi thub', 'https://github.com/a/b')).toBeUndefined();
	});
});

describe('buildProject / toView', () => {
	it('writes a contracts §2 Project owned by the login', () => {
		const input = mk({
			name: 'enzarb',
			displayName: 'Enzarb',
			repositories: [
				{ url: 'https://github.com/enzarb/enzarb-ui', connector: 'github', defaultBranch: 'dev' }
			],
			triggers: { issues: true, changeRequests: false, mentions: true }
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
		const p = buildProject(mk(), 'u');
		p.spec.displayName = '';
		expect(toView(p)).toMatchObject({ displayName: 'x', ready: false, namespace: null });
		p.status = { conditions: [{ type: 'Ready', status: 'True' }], namespace: 'enzarb-p-x' };
		expect(toView(p)).toMatchObject({ ready: true, namespace: 'enzarb-p-x' });
	});
});
