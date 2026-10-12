import type { RepoResult } from '#lib/repo-names.ts';
import type { Sql } from './db.ts';
import type { CreateRepoIntent } from './create-repo-intent.ts';
import {
	RepoCreateError,
	addRepoToInstallation,
	createRepository,
	installationCoversAll,
	sameOwner
} from './github-repos.ts';
import {
	getUserRepositorySnapshot,
	syncUserRepositories,
	waitForRepository
} from './user-repositories.ts';

type FetchFn = typeof fetch;

export interface RepoOutcome {
	code: RepoResult;
	/** GitHub's HTTP status when a GitHub call failed (a number, never free text). */
	status?: number;
	/** Key of the repository to preselect; set only when this flow created it (or may have). */
	key?: string;
}

/** Poll budget for the snapshot after creating; tests shrink it. */
export const pollDefaults = { tries: 5, delayMs: 2000 };

/**
 * The GitHub side of a create-repository callback. Runs only after the callback has verified the
 * state, the intent and the identity. `token` is the user-to-server token: it makes at most the
 * creation, the optional installation add and the snapshot reads, and is never stored or logged.
 * Whatever happens, the user's snapshot is re-synced once so sign-in stays current.
 */
export async function runCreateRepo(
	sql: Sql,
	userId: number,
	token: string,
	intent: CreateRepoIntent,
	fetchFn: FetchFn = fetch
): Promise<RepoOutcome> {
	let synced = false;
	const finish = async (outcome: RepoOutcome): Promise<RepoOutcome> => {
		if (!synced) await syncUserRepositories(sql, userId, token, fetchFn);
		console.log(`create-repo github:${intent.owner}/${intent.name} ${outcome.code}`.toLowerCase());
		return outcome;
	};

	// Re-validate the owner against this user's installations (the intent is only a request).
	const snapshot = await getUserRepositorySnapshot(sql, userId);
	const account = snapshot.repositories.find((r) => sameOwner(r.accountLogin, intent.owner));
	if (!account) return finish({ code: 'owner-not-allowed' });

	let created;
	try {
		created = await createRepository(
			token,
			{
				owner: account.accountLogin,
				selfLogin: intent.login,
				name: intent.name,
				private: intent.visibility === 'private',
				description: intent.description
			},
			fetchFn
		);
	} catch (e) {
		if (e instanceof RepoCreateError) {
			// A timeout may hide a created repository: let the form offer it.
			const key =
				e.code === 'timeout' ? `github:${account.accountLogin}/${intent.name}` : undefined;
			return finish({ code: e.code, status: e.status, key: key?.toLowerCase() });
		}
		return finish({ code: 'failed' });
	}

	const key = `github:${created.fullName}`.toLowerCase();
	// Only "selected repositories" installations need the explicit add.
	if ((await installationCoversAll(token, account.installationId, fetchFn)) === false) {
		try {
			await addRepoToInstallation(token, account.installationId, created.id, fetchFn);
		} catch (e) {
			// `synced` is still false, so `finish` does the single snapshot sync here.
			return finish({
				code: 'created-not-installed',
				status: e instanceof RepoCreateError ? e.status : undefined,
				key
			});
		}
	}
	synced = true; // waitForRepository syncs on every try
	const appeared = await waitForRepository(sql, userId, token, key, { ...pollDefaults, fetchFn });
	return finish({ code: appeared ? 'created' : 'timeout', key });
}
