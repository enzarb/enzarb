import { config } from './config';

async function giteaFetch(path: string, options?: RequestInit) {
	const token = process.env.GITEA_ADMIN_TOKEN ?? '';
	const res = await fetch(`${config.giteaUrl}/api/v1${path}`, {
		...options,
		headers: {
			'Content-Type': 'application/json',
			Authorization: `token ${token}`,
			...options?.headers
		}
	});
	if (!res.ok) throw new Error(`Gitea API error ${res.status}: ${await res.text()}`);
	return res.json();
}

export async function createRepo(orgSlug: string, repoName: string) {
	return giteaFetch(`/orgs/${orgSlug}/repos`, {
		method: 'POST',
		body: JSON.stringify({ name: repoName, auto_init: true, private: false })
	});
}

export async function listRepos(orgSlug: string) {
	return giteaFetch(`/orgs/${orgSlug}/repos`);
}

export async function getRepoContents(orgSlug: string, repo: string, path: string, ref = 'main') {
	return giteaFetch(`/repos/${orgSlug}/${repo}/contents/${encodeURIComponent(path)}?ref=${encodeURIComponent(ref)}`);
}

export async function listRepoTree(orgSlug: string, repo: string, path: string, ref = 'main') {
	return giteaFetch(`/repos/${orgSlug}/${repo}/git/trees/${encodeURIComponent(ref)}?recursive=false`);
}

export async function listBranches(orgSlug: string, repo: string) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/branches`);
}

export async function listActionsRuns(orgSlug: string, repo: string) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/actions/runs`);
}
