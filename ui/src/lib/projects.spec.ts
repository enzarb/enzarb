import { describe, expect, it } from 'vitest';
import * as v from 'valibot';
import {
	MaxRunsSchema,
	buildProject,
	normalizeRepoKey,
	pendingChangeSteps,
	toView,
	type NewProject
} from './projects.ts';

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

describe('MaxRunsSchema', () => {
	it('accepts whole numbers from 1 to 16', () => {
		for (const n of [1, 2, 16]) expect(v.safeParse(MaxRunsSchema, n).success).toBe(true);
	});
	it('rejects out-of-range, fractional and non-number input', () => {
		for (const x of [0, 17, 2.5, '2', null, NaN])
			expect(v.safeParse(MaxRunsSchema, x).success).toBe(false);
	});
});

describe('maxRuns in buildProject / toView', () => {
	it('is written only when given and exposed raw by the view', () => {
		expect('maxRuns' in buildProject(mk(), 'u').spec).toBe(false);
		const p = buildProject(mk({ maxRuns: 4 }), 'u');
		expect(p.spec.maxRuns).toBe(4);
		expect(toView(p).maxRuns).toBe(4);
		expect(toView(buildProject(mk(), 'u')).maxRuns).toBeUndefined();
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
		expect(toView(p)).toMatchObject({
			displayName: 'x',
			ready: false,
			namespace: null,
			sandboxName: null
		});
		p.status = {
			conditions: [{ type: 'Ready', status: 'True' }],
			namespace: 'enzarb-sandboxes',
			sandbox: { name: 'sb-x' }
		};
		expect(toView(p)).toMatchObject({
			ready: true,
			namespace: 'enzarb-sandboxes',
			sandboxName: 'sb-x'
		});
	});
});

describe('pendingChange', () => {
	const base = () => buildProject(mk(), 'u');
	const change = { generation: 2, since: '2026-10-12T03:00:00Z', reason: 'maxRuns 2 -> 4' };

	it('maps status.pendingChange', () => {
		const p = base();
		p.status = { pendingChange: change };
		expect(toView(p).pendingChange).toEqual(change);
	});
	it.each([undefined, null, {}, { since: '', reason: 'x' }, { since: 5, reason: 'x' }, 'x'])(
		'maps %j to no banner',
		(raw) => {
			const p = base();
			p.status = { pendingChange: raw as never };
			expect(toView(p).pendingChange).toBeNull();
		}
	);
	it('is null without a status', () => {
		expect(toView(base()).pendingChange).toBeNull();
	});
	it('falls back to the SandboxChangePending condition', () => {
		const p = base();
		p.status = {
			conditions: [
				{ type: 'SandboxChangePending', status: 'True', message: 'cpu', lastTransitionTime: 't' }
			]
		};
		expect(toView(p).pendingChange).toEqual({ generation: 0, since: 't', reason: 'cpu' });
		p.status = { conditions: [{ type: 'SandboxChangePending', status: 'False' }] };
		expect(toView(p).pendingChange).toBeNull();
	});
});

describe('pendingChangeSteps', () => {
	const change = { generation: 1, since: 't', reason: 'r' };
	const states = (steps: ReturnType<typeof pendingChangeSteps>) => steps.map((s) => s.state);

	it('waits for running jobs without presenting the count as exact', () => {
		const s = pendingChangeSteps(change, 3);
		expect(states(s)).toEqual(['done', 'active', 'pending', 'pending']);
		expect(s[1].label).toBe('Waiting for running jobs to finish');
		expect(pendingChangeSteps(change, 1)[1].label).toBe(s[1].label);
	});
	it('applies once no jobs run', () => {
		expect(states(pendingChangeSteps(change, 0))).toEqual(['done', 'done', 'active', 'pending']);
	});
	it('keeps waiting active while the job count is unknown', () => {
		const s = pendingChangeSteps(change, null);
		expect(states(s)).toEqual(['done', 'active', 'pending', 'pending']);
		expect(s[1].label).toBe('Checking running jobs');
	});
	it('is all done once applied', () => {
		expect(states(pendingChangeSteps(null, 0, true))).toEqual(['done', 'done', 'done', 'done']);
	});
	it('is all pending when never queued', () => {
		expect(states(pendingChangeSteps(null, 0))).toEqual([
			'pending',
			'pending',
			'pending',
			'pending'
		]);
	});
});
