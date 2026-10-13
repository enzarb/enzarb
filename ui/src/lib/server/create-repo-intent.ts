import { randomBytes } from 'node:crypto';
import { SignJWT, jwtVerify } from 'jose';
import type { Project } from '#lib/project-types.ts';
import { sameOwner, validateDescription, validateRepoName } from '#lib/repo-names.ts';
import { safeReturnPath } from './return-path.ts';
import { repoOwners, snapshotProblem } from './project-onboarding.ts';
import type { UserRepositorySnapshot } from './user-repositories.ts';

export const INTENT_TTL_SECONDS = 10 * 60;
const AUDIENCE = 'enzarb-create-repo-intent';

/** What the user asked for, validated and signed by `startCreateRepo`. Never holds a token. */
export interface CreateRepoIntent {
	/** Canonicalised to the snapshot's spelling of the account login. */
	owner: string;
	name: string;
	visibility: 'private' | 'public';
	description: string;
	/** Internal user id and the user's GitHub identity (what the callback must see again). */
	userId: number;
	githubId: number;
	login: string;
	nonce: string;
	/** Where to land afterwards (already through `safeReturnPath`). */
	ret: string;
}

export const newNonce = () => randomBytes(16).toString('base64url');

const key = (secret: string) => new TextEncoder().encode(secret);

export async function signIntent(
	secret: string,
	intent: CreateRepoIntent,
	nowSeconds = Math.floor(Date.now() / 1000)
): Promise<string> {
	return new SignJWT({ ...intent })
		.setProtectedHeader({ alg: 'HS256' })
		.setAudience(AUDIENCE)
		.setIssuedAt(nowSeconds)
		.setExpirationTime(nowSeconds + INTENT_TTL_SECONDS)
		.sign(key(secret));
}

/** The intent in a cookie value, or null when missing, tampered with, expired or malformed. */
export async function verifyIntent(
	secret: string,
	token: string | undefined,
	nowSeconds = Math.floor(Date.now() / 1000)
): Promise<CreateRepoIntent | null> {
	if (!token) return null;
	try {
		const { payload } = await jwtVerify(token, key(secret), {
			algorithms: ['HS256'],
			audience: AUDIENCE,
			currentDate: new Date(nowSeconds * 1000)
		});
		const p = payload as Record<string, unknown>;
		if (
			typeof p.owner !== 'string' ||
			typeof p.name !== 'string' ||
			(p.visibility !== 'private' && p.visibility !== 'public') ||
			typeof p.description !== 'string' ||
			typeof p.userId !== 'number' ||
			typeof p.githubId !== 'number' ||
			typeof p.login !== 'string' ||
			typeof p.nonce !== 'string' ||
			typeof p.ret !== 'string'
		)
			return null;
		return {
			owner: p.owner,
			name: p.name,
			visibility: p.visibility,
			description: p.description,
			userId: p.userId,
			githubId: p.githubId,
			login: p.login,
			nonce: p.nonce,
			ret: safeReturnPath(p.ret)
		};
	} catch {
		return null;
	}
}

/** The signed-in user's identity must be the one the token's GitHub profile belongs to. */
export function intentMatchesProfile(
	intent: Pick<CreateRepoIntent, 'githubId' | 'login'>,
	profile: { id: number; login: string }
): boolean {
	return intent.githubId === profile.id && sameOwner(intent.login, profile.login);
}

/** The state carries the intent's nonce, so the callback only acts on a flow this browser began. */
export const stateWithNonce = (state: string, nonce: string) => `${state}.${nonce}`;
export const stateHasNonce = (state: string, nonce: string) => state.endsWith(`.${nonce}`);

export interface CreateRepoRequest {
	owner: string;
	name: string;
	visibility: 'private' | 'public';
	description: string;
}

/**
 * Validates a create-repository request against the user's own snapshot and the cluster's
 * Projects. The owner is never taken as typed: it must be an `accountLogin` of the snapshot.
 */
export function buildIntent(
	deps: {
		user: { id: number; githubId: number; login: string };
		snapshot: UserRepositorySnapshot;
		projects: Project[];
		returnPath: string;
		now?: Date;
	},
	input: CreateRepoRequest
): { ok: true; intent: CreateRepoIntent } | { ok: false; message: string } {
	const problem = snapshotProblem(deps.snapshot, deps.now ?? new Date());
	if (problem) return { ok: false, message: problem.message };
	const account = deps.snapshot.repositories.find((r) => sameOwner(r.accountLogin, input.owner));
	if (!account) return { ok: false, message: 'Choose one of your GitHub App installations' };
	const name = input.name.trim();
	const bad = validateRepoName(name) ?? validateDescription(input.description);
	if (bad) return { ok: false, message: bad };
	const repoKey = `github:${account.accountLogin}/${name}`.toLowerCase();
	if (repoOwners(deps.projects).has(repoKey))
		return { ok: false, message: `${account.accountLogin}/${name} is already used by a project` };
	return {
		ok: true,
		intent: {
			owner: account.accountLogin,
			name,
			visibility: input.visibility,
			description: input.description,
			userId: deps.user.id,
			githubId: deps.user.githubId,
			login: deps.user.login,
			nonce: newNonce(),
			ret: safeReturnPath(deps.returnPath)
		}
	};
}
