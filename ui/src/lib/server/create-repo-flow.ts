import type { RepoResult } from '#lib/repo-names.ts';
import type { CreateRepoIntent } from './create-repo-intent.ts';
import {
	RepoCreateError,
	addRepoToInstallation,
	createRepository,
	installationCoversAll,
	sameOwner
} from './github-repos.ts';
import type { RepositoryAccess } from './user-repositories.ts';

type FetchFn = typeof fetch;

export interface RepoOutcome {
	code: RepoResult;
	/** GitHub's HTTP status when a GitHub call failed (a number, never free text). */
	status?: number;
	/** Key of the repository to preselect; set only when this flow created it (or may have). */
	key?: string;
}

/** Poll budget for the new repository to become visible to the App; tests shrink it. */
export const pollDefaults = { tries: 5, delayMs: 2000 };

/**
 * Polls the live access check until `key` is visible to the App with write access for `login`
 * (GitHub can lag). Bounded: `tries` checks, `delayMs` apart. Returns whether it appeared.
 */
async function waitForRepository(
	access: RepositoryAccess,
	login: string,
	key: string
): Promise<boolean> {
	for (let attempt = 0; attempt < pollDefaults.tries; attempt++) {
		if (attempt > 0) await new Promise((r) => setTimeout(r, pollDefaults.delayMs));
		if (await access.check(login, key)) return true;
	}
	return false;
}

/**
 * The GitHub side of a create-repository callback. Runs only after the callback has verified the
 * state, the intent and the identity. `token` is the user-to-server token: it makes at most the
 * creation and the optional installation add, and is never stored or logged. The owner is
 * re-validated against the user's live repository list, and the result is confirmed with the
 * live access check.
 */
export async function runCreateRepo(
	access: RepositoryAccess,
	token: string,
	intent: CreateRepoIntent,
	fetchFn: FetchFn = fetch
): Promise<RepoOutcome> {
	const finish = (outcome: RepoOutcome): RepoOutcome => {
		console.log(`create-repo github:${intent.owner}/${intent.name} ${outcome.code}`.toLowerCase());
		return outcome;
	};

	// Re-validate the owner against the installations the user can write to (the intent is only a
	// request). A GitHub failure refuses the owner.
	const list = await access.list(intent.login, { refresh: true }).catch(() => null);
	const account = list?.repositories.find((r) => sameOwner(r.accountLogin, intent.owner));
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
			return finish({
				code: 'created-not-installed',
				status: e instanceof RepoCreateError ? e.status : undefined,
				key
			});
		}
	}
	const appeared = await waitForRepository(access, intent.login, key);
	// Refresh the display cache so the picker lists the new repository.
	if (appeared) await access.list(intent.login, { refresh: true }).catch(() => null);
	return finish({ code: appeared ? 'created' : 'timeout', key });
}
