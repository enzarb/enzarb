import { createHash, randomBytes } from 'node:crypto';
import type { Cookies } from '@sveltejs/kit';
import { GitHub } from 'arctic';
import { getConfig } from './config.ts';
import type { Sql } from './db.ts';
import { REALTIME_COOKIE } from './realtime-jwt.ts';
import { OAUTH_RETURN_COOKIE } from './return-path.ts';

export const SESSION_COOKIE = 'enzarb_session';
export const OAUTH_STATE_COOKIE = 'enzarb_oauth_state';
export const SESSION_TTL_MS = 30 * 24 * 60 * 60 * 1000;
/** Sessions with less than this left are extended on use. */
const RENEW_BELOW_MS = SESSION_TTL_MS / 2;

export interface SessionUser {
	id: number;
	githubId: number;
	login: string;
	name: string | null;
	avatarUrl: string | null;
}

export interface Session {
	id: string;
	expiresAt: Date;
}

/** 160 random bits, base64url. Only ever held by the browser. */
export function generateSessionToken(): string {
	return randomBytes(20).toString('base64url');
}

/** The stored session id: hex sha-256 of the token. */
export function hashToken(token: string): string {
	return createHash('sha256').update(token).digest('hex');
}

export async function createSession(
	sql: Sql,
	userId: number,
	now = new Date()
): Promise<{ token: string; session: Session }> {
	const token = generateSessionToken();
	const session = { id: hashToken(token), expiresAt: new Date(now.getTime() + SESSION_TTL_MS) };
	await sql`INSERT INTO app.sessions (id, user_id, expires_at)
		VALUES (${session.id}, ${userId}, ${session.expiresAt})`;
	return { token, session };
}

/** Resolves a cookie token to its user, deleting expired sessions and sliding the expiry. */
export async function validateSessionToken(
	sql: Sql,
	token: string,
	now = new Date()
): Promise<{ user: SessionUser; session: Session } | null> {
	const id = hashToken(token);
	const [row] = await sql<
		{
			expires_at: Date;
			uid: string;
			github_id: string;
			login: string;
			name: string | null;
			avatar_url: string | null;
		}[]
	>`SELECT s.expires_at, u.id AS uid, u.github_id, u.login, u.name, u.avatar_url
		FROM app.sessions s JOIN app.users u ON u.id = s.user_id
		WHERE s.id = ${id}`;
	if (!row) return null;
	let expiresAt = row.expires_at;
	if (expiresAt.getTime() <= now.getTime()) {
		await sql`DELETE FROM app.sessions WHERE id = ${id}`;
		return null;
	}
	if (expiresAt.getTime() - now.getTime() < RENEW_BELOW_MS) {
		expiresAt = new Date(now.getTime() + SESSION_TTL_MS);
		await sql`UPDATE app.sessions SET expires_at = ${expiresAt} WHERE id = ${id}`;
	}
	return {
		session: { id, expiresAt },
		user: {
			id: Number(row.uid),
			githubId: Number(row.github_id),
			login: row.login,
			name: row.name,
			avatarUrl: row.avatar_url
		}
	};
}

export async function invalidateSession(sql: Sql, sessionId: string): Promise<void> {
	await sql`DELETE FROM app.sessions WHERE id = ${sessionId}`;
}

export interface GitHubProfile {
	id: number;
	login: string;
	name: string | null;
	avatarUrl: string | null;
}

export async function upsertUser(sql: Sql, p: GitHubProfile): Promise<number> {
	const [row] = await sql<{ id: string }[]>`
		INSERT INTO app.users (github_id, login, name, avatar_url)
		VALUES (${p.id}, ${p.login}, ${p.name}, ${p.avatarUrl})
		ON CONFLICT (github_id) DO UPDATE
			SET login = EXCLUDED.login, name = EXCLUDED.name,
			    avatar_url = EXCLUDED.avatar_url, updated_at = now()
		RETURNING id`;
	return Number(row.id);
}

function cookieOptions(secure: boolean, expires?: Date) {
	return { path: '/', httpOnly: true, sameSite: 'lax', secure, expires } as const;
}

export function setSessionCookie(cookies: Cookies, token: string, expiresAt: Date): void {
	cookies.set(SESSION_COOKIE, token, cookieOptions(isSecure(), expiresAt));
}

/** The Path must match the one it was set with or the delete does nothing. */
export function deleteRealtimeCookie(cookies: Cookies): void {
	cookies.delete(REALTIME_COOKIE, { path: '/live/' });
}

export function deleteSessionCookie(cookies: Cookies): void {
	cookies.delete(SESSION_COOKIE, { path: '/' });
	deleteRealtimeCookie(cookies);
}

export function setOauthStateCookie(cookies: Cookies, state: string): void {
	cookies.set(OAUTH_STATE_COOKIE, state, {
		...cookieOptions(isSecure()),
		maxAge: 10 * 60
	});
}

/** Marks a sign-in round trip as a create-repository one (set only by `startCreateRepo`'s flow). */
export const OAUTH_PURPOSE_COOKIE = 'enzarb_oauth_purpose';
export const CREATE_REPO_INTENT_COOKIE = 'enzarb_create_repo';
export const CREATE_REPO_PURPOSE = 'create-repo';

export function setOauthPurposeCookie(cookies: Cookies, purpose: string): void {
	cookies.set(OAUTH_PURPOSE_COOKIE, purpose, { ...cookieOptions(isSecure()), maxAge: 10 * 60 });
}

export function setCreateRepoIntentCookie(cookies: Cookies, signed: string): void {
	cookies.set(CREATE_REPO_INTENT_COOKIE, signed, { ...cookieOptions(isSecure()), maxAge: 10 * 60 });
}

export function deleteCreateRepoIntentCookie(cookies: Cookies): void {
	cookies.delete(CREATE_REPO_INTENT_COOKIE, { path: '/' });
}

function isSecure(): boolean {
	return getConfig().publicUrl.startsWith('https:');
}

export function setOauthReturnCookie(cookies: Cookies, path: string): void {
	cookies.set(OAUTH_RETURN_COOKIE, path, { ...cookieOptions(isSecure()), maxAge: 10 * 60 });
}

export function githubClient(): GitHub {
	const c = getConfig();
	return new GitHub(
		c.githubOauthClientId,
		c.githubOauthClientSecret,
		`${c.publicUrl}/login/github/callback`
	);
}

/** Fetches the authenticated user's profile with an OAuth access token. */
export async function fetchGitHubProfile(accessToken: string): Promise<GitHubProfile> {
	const res = await fetch('https://api.github.com/user', {
		headers: { Authorization: `Bearer ${accessToken}`, 'User-Agent': 'enzarb-ui' }
	});
	if (!res.ok) throw new Error(`GitHub /user returned ${res.status}`);
	const u = (await res.json()) as {
		id: number;
		login: string;
		name: string | null;
		avatar_url: string | null;
	};
	return { id: u.id, login: u.login, name: u.name, avatarUrl: u.avatar_url };
}
