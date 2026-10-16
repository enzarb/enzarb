import { getConfig } from './config.ts';
import { createRepositoryAccess, type RepositoryAccess } from './user-repositories.ts';

let shared: RepositoryAccess | undefined;

/** The process's live repository source (its display cache is shared by requests on this replica). */
export function getRepositoryAccess(): RepositoryAccess {
	if (!shared) {
		const { githubAppId, githubAppPrivateKey } = getConfig();
		shared = createRepositoryAccess({ appId: githubAppId, privateKey: githubAppPrivateKey });
	}
	return shared;
}
