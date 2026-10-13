import { describe, expect, it } from 'vitest';
import type { Project } from '#lib/project-types.ts';
import {
	buildIntent,
	intentMatchesProfile,
	signIntent,
	stateHasNonce,
	stateWithNonce,
	verifyIntent,
	type CreateRepoIntent
} from './create-repo-intent.ts';
import type { UserRepositorySnapshot } from './user-repositories.ts';

const SECRET = 'test-secret';
const NOW = new Date('2026-06-10T12:00:00Z');
const nowSeconds = Math.floor(NOW.getTime() / 1000);

const snapshot: UserRepositorySnapshot = {
	fetchedAt: new Date(NOW.getTime() - 3600_000),
	repositories: [
		{
			repo: 'github:acme/web',
			installationId: 1,
			accountLogin: 'Acme',
			private: true,
			defaultBranch: 'main'
		},
		{
			repo: 'github:octo/own',
			installationId: 2,
			accountLogin: 'octo',
			private: false,
			defaultBranch: 'main'
		}
	]
};
const user = { id: 1, githubId: 99, login: 'octo' };
const request = {
	owner: 'acme',
	name: 'new-repo',
	visibility: 'private' as const,
	description: ''
};
const projectHolding = (url: string): Project => ({
	apiVersion: 'enzarb.dev/v1alpha1',
	kind: 'Project',
	metadata: { name: 'p' },
	spec: {
		owner: 'someone',
		displayName: 'p',
		repositories: [{ url, connector: 'github', defaultBranch: 'main' }],
		runMode: 'worktree',
		triggers: { issues: true, changeRequests: true, mentions: true }
	}
});
const build = (over: Partial<typeof request> = {}, projects: Project[] = []) =>
	buildIntent(
		{ user, snapshot, projects, returnPath: '/projects/new', now: NOW },
		{ ...request, ...over }
	);

describe('buildIntent (startCreateRepo validation)', () => {
	it('accepts an owner that differs only in case and canonicalises it', () => {
		const r = build({ owner: 'ACME' });
		expect(r).toMatchObject({
			ok: true,
			intent: { owner: 'Acme', userId: 1, githubId: 99, login: 'octo' }
		});
		expect(r.ok && r.intent.nonce.length).toBeGreaterThan(10);
	});
	it('rejects an owner outside the installations and a free-text owner', () => {
		expect(build({ owner: 'evil-org' }).ok).toBe(false);
		expect(build({ owner: 'acme/../evil' }).ok).toBe(false);
		expect(build({ owner: '' }).ok).toBe(false);
	});
	it('rejects bad names and descriptions', () => {
		expect(build({ name: 'a/b' }).ok).toBe(false);
		expect(build({ name: 'a b' }).ok).toBe(false);
		expect(build({ name: '..' }).ok).toBe(false);
		expect(build({ description: 'x'.repeat(351) }).ok).toBe(false);
		expect(build({ description: 'a\nb' }).ok).toBe(false);
	});
	it('rejects a repository already held by a Project', () => {
		const r = build({ name: 'Web' }, [projectHolding('https://github.com/acme/web')]);
		expect(r).toMatchObject({ ok: false });
	});
	it('refuses a stale snapshot', () => {
		const stale = { ...snapshot, fetchedAt: new Date(NOW.getTime() - 48 * 3600_000) };
		const r = buildIntent(
			{ user, snapshot: stale, projects: [], returnPath: '/', now: NOW },
			request
		);
		expect(r.ok).toBe(false);
	});
	it('runs the return path through safeReturnPath', () => {
		const r = buildIntent(
			{ user, snapshot, projects: [], returnPath: 'https://evil.example', now: NOW },
			request
		);
		expect(r.ok && r.intent.ret).toBe('/');
	});
});

describe('signed intent', () => {
	const intent = (): CreateRepoIntent => {
		const r = build();
		if (!r.ok) throw new Error('setup');
		return r.intent;
	};

	it('round-trips', async () => {
		const i = intent();
		expect(await verifyIntent(SECRET, await signIntent(SECRET, i, nowSeconds), nowSeconds)).toEqual(
			i
		);
	});
	it('refuses a tampered, foreign-signed, expired or missing cookie', async () => {
		const token = await signIntent(SECRET, intent(), nowSeconds);
		const [h, p, s] = token.split('.');
		const forged = Buffer.from(
			JSON.stringify({ ...JSON.parse(Buffer.from(p, 'base64url').toString()), owner: 'evil' })
		).toString('base64url');
		expect(await verifyIntent(SECRET, `${h}.${forged}.${s}`, nowSeconds)).toBeNull();
		expect(await verifyIntent('other-secret', token, nowSeconds)).toBeNull();
		expect(await verifyIntent(SECRET, token, nowSeconds + 601)).toBeNull();
		expect(await verifyIntent(SECRET, undefined, nowSeconds)).toBeNull();
		expect(await verifyIntent(SECRET, 'garbage', nowSeconds)).toBeNull();
	});
	it('is not accepted from a session token of the same secret', async () => {
		const { mintSessionToken } = await import('./session-jwt.ts');
		expect(await verifyIntent(SECRET, await mintSessionToken(SECRET, 'octo', 'p'))).toBeNull();
	});
});

describe('identity and state binding', () => {
	const i = { githubId: 99, login: 'octo' };
	it('matches by id and login, case-insensitively', () => {
		expect(intentMatchesProfile(i, { id: 99, login: 'Octo' })).toBe(true);
	});
	it('refuses another id even with the same login, and another login with the same id', () => {
		expect(intentMatchesProfile(i, { id: 100, login: 'octo' })).toBe(false);
		expect(intentMatchesProfile(i, { id: 99, login: 'someone' })).toBe(false);
	});
	it('binds the state to the nonce', () => {
		expect(stateHasNonce(stateWithNonce('abc', 'n1'), 'n1')).toBe(true);
		expect(stateHasNonce(stateWithNonce('abc', 'n1'), 'n2')).toBe(false);
		expect(stateHasNonce('abc', 'n1')).toBe(false);
	});
});
