import { error } from '@sveltejs/kit';
import { getRequestEvent } from '$app/server';
import { listProjects, getProject, createProject, deleteProject } from '$lib/k8s';
import { sql } from '$lib/db';
import { tiers } from '$lib/config';
import { mintProjectToken } from '$lib/jwt';
import { z } from 'zod/v4';

function requireSession() {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	return locals.session;
}

async function requireOrgMember(orgId: string, minRole?: 'admin' | 'owner') {
	const session = requireSession();
	const org = session.orgs.find((o) => o.id === orgId);
	if (!org) error(403, 'Not a member of this organization');
	if (minRole === 'admin' && org.role === 'member') error(403, 'Admin required');
	return org;
}

async function getOrgTier(orgId: string) {
	const rows = await sql`SELECT tier FROM organizations WHERE id = ${orgId}`;
	return (rows[0]?.tier ?? 'free') as keyof typeof tiers;
}

export async function getProjects(orgId: string) {
	await requireOrgMember(orgId);
	return listProjects(orgId);
}

export async function getProjectDetail(orgId: string, slug: string) {
	await requireOrgMember(orgId);
	return getProject(orgId, slug);
}

const CreateProjectSchema = z.object({
	orgId: z.string(),
	slug: z.string().min(1).max(63).regex(/^[a-z0-9-]+$/, 'Slug must be lowercase alphanumeric with dashes'),
	displayName: z.string().min(1),
	tools: z.array(z.object({ name: z.string(), version: z.string().default('latest') })),
	storageGi: z.number().int().min(1).default(10)
});

export async function createNewProject(input: z.infer<typeof CreateProjectSchema>) {
	const parsed = CreateProjectSchema.parse(input);
	await requireOrgMember(parsed.orgId, 'admin');

	// Enforce tier limits
	const tier = await getOrgTier(parsed.orgId);
	const limits = tiers[tier];
	const existing = await listProjects(parsed.orgId);
	if (existing.length >= limits.maxProjects) {
		error(422, `Free tier limited to ${limits.maxProjects} project(s). Upgrade to create more.`);
	}
	if (parsed.storageGi > limits.maxPvcGi) {
		error(422, `Storage exceeds tier limit of ${limits.maxPvcGi}Gi`);
	}

	return createProject(parsed.orgId, parsed);
}

export async function removeProject(orgId: string, slug: string) {
	await requireOrgMember(orgId, 'admin');
	return deleteProject(orgId, slug);
}

export async function getAgentToken(orgId: string, projectSlug: string) {
	const session = requireSession();
	await requireOrgMember(orgId);

	// Get project UID for token claims
	const project = await getProject(orgId, projectSlug) as any;
	const projectId = project.metadata?.uid;
	if (!projectId) error(404, 'Project not found');

	return mintProjectToken(session.userId, projectId, [
		'files:read', 'files:write', 'processes:manage', 'terminal'
	]);
}
