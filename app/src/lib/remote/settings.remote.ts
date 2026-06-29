import { query, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { sql } from '$lib/db';
import { encrypt, decrypt } from '$lib/crypto';
import { orgNamespace, createOrPatchSecret, deleteSecret, listProjects, restartWorkspacesForOrgs } from '$lib/k8s';
import { config } from '$lib/config';

function requireSession() {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	return locals.session;
}

// Sync the user-level env secrets K8s Secret for all orgs the user belongs to,
// then restart running workspaces so they pick up the updated envFrom values.
async function syncUserSecrets(userId: string, secrets: Record<string, string>) {
	const session = (await import('$app/server').then(m => m.getRequestEvent())).locals.session;
	if (!session) return;
	for (const org of session.orgs) {
		const ns = orgNamespace(org.id);
		const secretName = `${org.id}-user-env-secrets`;
		if (Object.keys(secrets).length === 0) {
			await deleteSecret(ns, secretName);
		} else {
			await createOrPatchSecret(ns, secretName, secrets);
		}
	}
	await restartWorkspacesForOrgs(session.orgs.map(o => o.id));
}

// Load all user secrets as a plain object (for K8s sync).
async function loadUserSecretMap(userId: string): Promise<Record<string, string>> {
	const rows = await sql<{ key: string; value: string }[]>`
		SELECT key, value FROM user_secrets WHERE user_id = ${userId}
	`;
	return Object.fromEntries(rows.map(r => [r.key, decrypt(r.value)]));
}

// Load all project secrets as a plain object (for K8s sync).
async function loadProjectSecretMap(projectId: string): Promise<Record<string, string>> {
	const rows = await sql<{ key: string; value: string }[]>`
		SELECT key, value FROM project_secrets WHERE project_id = ${projectId}
	`;
	return Object.fromEntries(rows.map(r => [r.key, r.value]));
}

// ── User secrets ─────────────────────────────────────────────────────────────

export const getUserSecrets = query(async () => {
	const session = requireSession();
	const rows = await sql<{ key: string; created_at: Date }[]>`
		SELECT key, created_at FROM user_secrets WHERE user_id = ${session.userId} ORDER BY key
	`;
	return rows;
});

export const setUserSecret = command(
	z.object({ key: z.string().min(1).max(256), value: z.string().max(65536) }),
	async ({ key, value }) => {
		const session = requireSession();
		await sql`
			INSERT INTO user_secrets (user_id, key, value) VALUES (${session.userId}, ${key}, ${encrypt(value)})
			ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value
		`;
		const map = await loadUserSecretMap(session.userId);
		await syncUserSecrets(session.userId, map);
	}
);

export const deleteUserSecret = command(
	z.object({ key: z.string() }),
	async ({ key }) => {
		const session = requireSession();
		await sql`DELETE FROM user_secrets WHERE user_id = ${session.userId} AND key = ${key}`;
		const map = await loadUserSecretMap(session.userId);
		await syncUserSecrets(session.userId, map);
	}
);

// ── Project secrets ───────────────────────────────────────────────────────────

export const getProjectSecrets = query(async () => {
	const { params, locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const org = locals.session.orgs.find(o => o.slug === params.namespace);
	if (!org) error(403, 'Not a member of this organization');

	// Resolve project UID from slug.
	const { getProject } = await import('$lib/k8s');
	const project = await getProject(org.id, params.project!) as { metadata?: { uid?: string } };
	const projectId = project?.metadata?.uid;
	if (!projectId) error(404, 'Project not found');

	const rows = await sql<{ key: string; created_at: Date }[]>`
		SELECT key, created_at FROM project_secrets WHERE project_id = ${projectId} ORDER BY key
	`;
	return rows;
});

export const setProjectSecret = command(
	z.object({ key: z.string().min(1).max(256), value: z.string().max(65536) }),
	async ({ key, value }) => {
		const { params, locals } = getRequestEvent();
		if (!locals.session) error(401, 'Unauthorized');
		const org = locals.session.orgs.find(o => o.slug === params.namespace);
		if (!org) error(403, 'Not a member of this organization');

		const { getProject } = await import('$lib/k8s');
		const project = await getProject(org.id, params.project!) as { metadata?: { uid?: string; name?: string } };
		const projectId = project?.metadata?.uid;
		const projectSlug = project?.metadata?.name;
		if (!projectId || !projectSlug) error(404, 'Project not found');

		await sql`
			INSERT INTO project_secrets (project_id, key, value) VALUES (${projectId}, ${key}, ${value})
			ON CONFLICT (project_id, key) DO UPDATE SET value = EXCLUDED.value
		`;
		const map = await loadProjectSecretMap(projectId);
		await createOrPatchSecret(orgNamespace(org.id), `${projectSlug}-project-env-secrets`, map);
	}
);

export const deleteProjectSecret = command(
	z.object({ key: z.string() }),
	async ({ key }) => {
		const { params, locals } = getRequestEvent();
		if (!locals.session) error(401, 'Unauthorized');
		const org = locals.session.orgs.find(o => o.slug === params.namespace);
		if (!org) error(403, 'Not a member of this organization');

		const { getProject } = await import('$lib/k8s');
		const project = await getProject(org.id, params.project!) as { metadata?: { uid?: string; name?: string } };
		const projectId = project?.metadata?.uid;
		const projectSlug = project?.metadata?.name;
		if (!projectId || !projectSlug) error(404, 'Project not found');

		await sql`DELETE FROM project_secrets WHERE project_id = ${projectId} AND key = ${key}`;
		const map = await loadProjectSecretMap(projectId);
		if (Object.keys(map).length === 0) {
			await deleteSecret(orgNamespace(org.id), `${projectSlug}-project-env-secrets`);
		} else {
			await createOrPatchSecret(orgNamespace(org.id), `${projectSlug}-project-env-secrets`, map);
		}
	}
);

// ── GitHub OAuth ──────────────────────────────────────────────────────────────

// Whether GitHub OAuth is configured on this platform.
export const getGithubOAuthConfig = query(async () => {
	return { enabled: !!config.githubOAuthClientId };
});

// Check if the current user has GitHub connected (has GH_TOKEN in user_secrets).
export const getGithubConnection = query(async () => {
	const session = requireSession();
	const rows = await sql<{ key: string }[]>`
		SELECT key FROM user_secrets WHERE user_id = ${session.userId} AND key = 'ENZARB_GIT_USER_NAME'
	`;
	if (rows.length === 0) return null;
	const nameRow = await sql<{ value: string }[]>`
		SELECT value FROM user_secrets WHERE user_id = ${session.userId} AND key = 'ENZARB_GIT_USER_NAME'
	`;
	return { login: nameRow[0] ? decrypt(nameRow[0].value) : '' };
});

export const disconnectGithub = command(async () => {
	const session = requireSession();
	const githubKeys = ['GH_TOKEN', 'GITHUB_TOKEN', 'ENZARB_GIT_USER_NAME', 'ENZARB_GIT_USER_EMAIL'];
	await sql`
		DELETE FROM user_secrets WHERE user_id = ${session.userId} AND key = ANY(${githubKeys})
	`;
	const map = await loadUserSecretMap(session.userId);
	await syncUserSecrets(session.userId, map);
});
