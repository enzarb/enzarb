import { error } from '@sveltejs/kit';
import { getRequestEvent } from '$app/server';
import { getRepoContents } from '$lib/gitea';

function requireSession() {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	return locals.session;
}

async function requireOrgMember(orgId: string) {
	const session = requireSession();
	if (!session.orgs.find((o) => o.id === orgId)) error(403, 'Forbidden');
}

export async function getGitTree(orgId: string, projectSlug: string, path: string, ref = 'main') {
	await requireOrgMember(orgId);
	return getRepoContents(orgId, projectSlug, path, ref);
}
