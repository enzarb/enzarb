import { query, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import {
	listProjects,
	getProject as k8sGetProject,
	createProject as k8sCreateProject,
	softDeleteProject,
	recoverProject as k8sRecoverProject,
	purgeAfterOf,
	isOrgReady
} from '$lib/k8s';
import { sql } from '$lib/db';
import { tiers, type TierConfig } from '$lib/config';
import { getSettings } from '$lib/settings';
import { mintProjectToken } from '$lib/jwt';

function requireSession() {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	return locals.session;
}

function resolveNamespace(minRole?: 'admin') {
	const { locals, params } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const org = locals.session.orgs.find((o) => o.slug === params.namespace);
	if (!org) error(403, 'Not a member of this organization');
	if (minRole === 'admin' && org.role === 'member') error(403, 'Admin required');
	return org;
}

async function getOrgTierValue(orgId: string) {
	const rows = await sql`SELECT tier FROM organizations WHERE id = ${orgId}`;
	return (rows[0]?.tier ?? 'free') as keyof typeof tiers;
}

// Resolve tier limits, applying admin-configurable overrides (currently the
// free-tier max workspace storage) on top of the static tier defaults.
async function resolveTierLimits(tier: keyof typeof tiers): Promise<TierConfig> {
	const limits = { ...tiers[tier] };
	if (tier === 'free') {
		const settings = await getSettings();
		limits.maxPvcGi = settings.freeMaxPvcGi;
	}
	return limits;
}

export const getProjects = query(async () => {
	const org = resolveNamespace();
	const projects = await listProjects(org.id);
	// Hide soft-deleted projects from the normal listing.
	return projects.filter((p: { metadata?: { annotations?: Record<string, string> } }) => !purgeAfterOf(p));
});

// Soft-deleted projects still within their recovery window.
export const getDeletedProjects = query(async () => {
	const org = resolveNamespace();
	const projects = await listProjects(org.id);
	return projects
		.filter((p: { metadata?: { annotations?: Record<string, string> } }) => purgeAfterOf(p))
		.map((p: { metadata?: { name?: string; annotations?: Record<string, string> } }) => ({
			slug: p.metadata?.name,
			purgeAfter: purgeAfterOf(p)
		}));
});

export const getProject = query(async () => {
	const { params } = getRequestEvent();
	const org = resolveNamespace();
	const project = (await k8sGetProject(org.id, params.project!)) as any;
	if (!project) error(404, 'Project not found');
	return project;
});

export const getOrgTierInfo = query(async () => {
	const org = resolveNamespace();
	const tier = await getOrgTierValue(org.id);
	return { tier, limits: await resolveTierLimits(tier) };
});

export const getAgentToken = query(async () => {
	const { params } = getRequestEvent();
	const session = requireSession();
	const org = resolveNamespace();
	const project = (await k8sGetProject(org.id, params.project!)) as any;
	const projectId = project?.metadata?.uid;
	if (!projectId) error(404, 'Project not found');
	return mintProjectToken(session.userId, projectId, [
		'files:read',
		'files:write',
		'processes:manage',
		'terminal'
	]);
});

const CreateProjectSchema = z.object({
	slug: z.string().min(1).max(63).regex(/^[a-z0-9-]+$/),
	displayName: z.string().min(1),
	tools: z.array(z.string()).default([]),
	storageGi: z.coerce.number().int().min(1).default(10)
});

export const createProject = command(
	CreateProjectSchema,
	async ({ slug, displayName, tools, storageGi }) => {
		const org = resolveNamespace('admin');

		const tier = await getOrgTierValue(org.id);
		const limits = await resolveTierLimits(tier);
		const existing = await listProjects(org.id);
		if (existing.length >= limits.maxProjects) {
			error(422, `Free tier limited to ${limits.maxProjects} project(s). Upgrade to create more.`);
		}
		if (storageGi > limits.maxPvcGi) {
			error(422, `Storage exceeds tier limit of ${limits.maxPvcGi}Gi`);
		}

		// The operator owns the org namespace via the Organization CR. If it isn't
		// Ready yet (brand-new org racing project creation), ask the client to retry
		// rather than letting the namespaced create 404.
		if (!(await isOrgReady(org.id))) {
			error(503, 'Workspace is still provisioning — please try again in a moment.');
		}

		await k8sCreateProject(org.id, {
			slug,
			displayName,
			tools: tools.map((name) => ({ name, version: 'latest' })),
			storageGi
		});
		return { slug };
	}
);

export const removeProject = command(z.object({ slug: z.string() }), async ({ slug }) => {
	const org = resolveNamespace('admin');
	const { retentionDays } = await getSettings();
	// Soft delete: recoverable for retentionDays before the operator purges it.
	return softDeleteProject(org.id, slug, retentionDays);
});

export const recoverProjectCommand = command(z.object({ slug: z.string() }), async ({ slug }) => {
	const org = resolveNamespace('admin');
	return k8sRecoverProject(org.id, slug);
});
