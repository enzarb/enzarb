import { error } from '@sveltejs/kit';
import { getRequestEvent } from '$app/server';
import { listRepositories, listTags, getManifest, deleteManifest } from '$lib/zot';

function requireSession() {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	return locals.session;
}

async function requireOrgMember(orgId: string) {
	const session = requireSession();
	if (!session.orgs.find((o) => o.id === orgId)) error(403, 'Forbidden');
}

export async function getRepositories(orgId: string) {
	await requireOrgMember(orgId);
	const all = await listRepositories();
	// Filter to repos belonging to this org (prefix match)
	return all.filter((r) => r.name.startsWith(`${orgId}/`));
}

export async function getRepoTags(orgId: string, repo: string) {
	await requireOrgMember(orgId);
	if (!repo.startsWith(`${orgId}/`)) error(403, 'Forbidden');
	return listTags(repo);
}

export async function getImageManifest(orgId: string, repo: string, reference: string) {
	await requireOrgMember(orgId);
	if (!repo.startsWith(`${orgId}/`)) error(403, 'Forbidden');
	return getManifest(repo, reference);
}

export async function removeImage(orgId: string, repo: string, digest: string) {
	const session = requireSession();
	const org = session.orgs.find((o) => o.id === orgId);
	if (!org) error(403, 'Forbidden');
	if (org.role === 'member') error(403, 'Admin required');
	if (!repo.startsWith(`${orgId}/`)) error(403, 'Forbidden');
	await deleteManifest(repo, digest);
}
