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
	if (!res.ok) {
		if (res.status === 404) return null;
		throw new Error(`Gitea API error ${res.status}: ${await res.text()}`);
	}
	if (res.status === 204) return null;
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

export async function listTags(orgSlug: string, repo: string) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/tags`) as Promise<{ name: string }[] | null>;
}

export async function listIssues(orgSlug: string, repo: string, state: 'open' | 'closed' | 'all' = 'open', page = 1, limit = 20) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/issues?type=issues&state=${state}&page=${page}&limit=${limit}`) as Promise<any[] | null>;
}

export async function getIssue(orgSlug: string, repo: string, index: number) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/issues/${index}`) as Promise<any | null>;
}

export async function createIssue(orgSlug: string, repo: string, title: string, body: string) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/issues`, {
		method: 'POST',
		body: JSON.stringify({ title, body })
	});
}

export async function editIssue(orgSlug: string, repo: string, index: number, fields: { title?: string; body?: string; state?: 'open' | 'closed' }) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/issues/${index}`, {
		method: 'PATCH',
		body: JSON.stringify(fields)
	});
}

export async function listIssueComments(orgSlug: string, repo: string, index: number) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/issues/${index}/comments`) as Promise<any[] | null>;
}

export async function createIssueComment(orgSlug: string, repo: string, index: number, body: string) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/issues/${index}/comments`, {
		method: 'POST',
		body: JSON.stringify({ body })
	});
}

export async function listIssueLabels(orgSlug: string, repo: string) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/labels`) as Promise<{ id: number; name: string; color: string }[] | null>;
}

export async function createLabel(orgSlug: string, repo: string, name: string, color: string) {
	return giteaFetch(`/repos/${orgSlug}/${repo}/labels`, {
		method: 'POST',
		body: JSON.stringify({ name, color })
	});
}

export type GiteaCommit = {
	sha: string;
	commit: { message: string; author: { name: string; date: string } };
	html_url: string;
};

export type GiteaBlameSection = {
	commit: GiteaCommit;
	lines: string[];
};

export async function listCommits(orgSlug: string, repo: string, sha = 'main', path?: string, page = 1, limit = 30): Promise<GiteaCommit[] | null> {
	const q = new URLSearchParams({ sha, page: String(page), limit: String(limit) });
	if (path) q.set('path', path);
	return giteaFetch(`/repos/${orgSlug}/${repo}/commits?${q}`) as Promise<GiteaCommit[] | null>;
}

export async function getCommit(orgSlug: string, repo: string, sha: string): Promise<any | null> {
	return giteaFetch(`/repos/${orgSlug}/${repo}/git/commits/${sha}`);
}

export async function getBlame(orgSlug: string, repo: string, filepath: string, ref = 'main'): Promise<GiteaBlameSection[] | null> {
	return giteaFetch(`/repos/${orgSlug}/${repo}/git/blame/${encodeURIComponent(filepath)}?ref=${encodeURIComponent(ref)}`) as Promise<GiteaBlameSection[] | null>;
}
