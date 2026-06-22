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
	forceRestartWorkspace,
	purgeAfterOf,
	isOrgReady
} from '$lib/k8s';
import { sql } from '$lib/db';
import { tiers, type TierConfig } from '$lib/config';
import { getSettings } from '$lib/settings';
import { mintProjectToken } from '$lib/jwt';
import { resolveOrg, requirePrivilege } from './guard';

function requireSession() {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	return locals.session;
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
	const org = resolveOrg();
	const projects = await listProjects(org.id);
	// Hide soft-deleted projects from the normal listing.
	return projects.filter((p: { metadata?: { annotations?: Record<string, string> } }) => !purgeAfterOf(p));
});

// Soft-deleted projects still within their recovery window.
export const getDeletedProjects = query(async () => {
	const org = resolveOrg();
	const projects = await listProjects(org.id);
	return projects
		.filter((p: { metadata?: { annotations?: Record<string, string> } }) => purgeAfterOf(p))
		.map((p: { metadata?: { name?: string; annotations?: Record<string, string> } }) => ({
			slug: p.metadata?.name,
			purgeAfter: purgeAfterOf(p)
		}));
});

export const getProject = query(z.string().optional(), async (slug) => {
	const { params } = getRequestEvent();
	const org = resolveOrg();
	const project = (await k8sGetProject(org.id, slug ?? params.project!)) as any;
	if (!project) error(404, 'Project not found');
	return project;
});

export const getOrgTierInfo = query(async () => {
	const org = resolveOrg();
	const tier = await getOrgTierValue(org.id);
	return { tier, limits: await resolveTierLimits(tier) };
});

export const getAgentToken = query(async () => {
	const { params } = getRequestEvent();
	const session = requireSession();
	const org = resolveOrg();
	const project = (await k8sGetProject(org.id, params.project!)) as any;
	const projectId = project?.metadata?.uid;
	if (!projectId) error(404, 'Project not found');
	return mintProjectToken(session.userId, projectId, [
		'files:read',
		'files:write',
		'processes:manage',
		'terminal',
		'tools:manage'
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
		const org = requirePrivilege('project.create');

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

// Recoverable soft delete, gated on the project.delete privilege (granted to the
// member role by default). Reversible within the retention window.
export const removeProject = command(z.object({ slug: z.string() }), async ({ slug }) => {
	const org = requirePrivilege('project.delete');
	const { retentionDays } = await getSettings();
	// Soft delete: recoverable for retentionDays before the operator purges it.
	return softDeleteProject(org.id, slug, retentionDays);
});

export const recoverProjectCommand = command(z.object({ slug: z.string() }), async ({ slug }) => {
	const org = requirePrivilege('project.delete');
	return k8sRecoverProject(org.id, slug);
});

export const restartWorkspace = command(z.object({ slug: z.string() }), async ({ slug }) => {
	const org = resolveOrg();
	await forceRestartWorkspace(org.id, slug);
});
