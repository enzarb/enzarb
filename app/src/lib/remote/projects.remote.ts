import { query, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import {
	listProjects,
	getProject as k8sGetProject,
	createProject as k8sCreateProject,
	deleteProject
} from '$lib/k8s';
import { sql } from '$lib/db';
import { tiers } from '$lib/config';
import { mintProjectToken } from '$lib/jwt';

function requireSession() {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	return locals.session;
}

function requireOrgMember(orgId: string, minRole?: 'admin') {
	const session = requireSession();
	const org = session.orgs.find((o) => o.id === orgId);
	if (!org) error(403, 'Not a member of this organization');
	if (minRole === 'admin' && org.role === 'member') error(403, 'Admin required');
	return org;
}

async function getOrgTierValue(orgId: string) {
	const rows = await sql`SELECT tier FROM organizations WHERE id = ${orgId}`;
	return (rows[0]?.tier ?? 'free') as keyof typeof tiers;
}

export const getProjects = query(async () => {
	const { params } = getRequestEvent();
	requireOrgMember(params.org!);
	return listProjects(params.org!);
});

export const getProject = query(async () => {
	const { params } = getRequestEvent();
	requireOrgMember(params.org!);
	const project = (await k8sGetProject(params.org!, params.project!)) as any;
	if (!project) error(404, 'Project not found');
	return project;
});

export const getOrgTierInfo = query(async () => {
	const { params } = getRequestEvent();
	requireOrgMember(params.org!);
	const tier = await getOrgTierValue(params.org!);
	return { tier, limits: tiers[tier] };
});

export const getAgentToken = query(async () => {
	const { params } = getRequestEvent();
	const session = requireSession();
	requireOrgMember(params.org!);
	const project = (await k8sGetProject(params.org!, params.project!)) as any;
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
		const { params } = getRequestEvent();
		requireOrgMember(params.org!, 'admin');

		const tier = await getOrgTierValue(params.org!);
		const limits = tiers[tier];
		const existing = await listProjects(params.org!);
		if (existing.length >= limits.maxProjects) {
			error(422, `Free tier limited to ${limits.maxProjects} project(s). Upgrade to create more.`);
		}
		if (storageGi > limits.maxPvcGi) {
			error(422, `Storage exceeds tier limit of ${limits.maxPvcGi}Gi`);
		}

		await k8sCreateProject(params.org!, {
			slug,
			displayName,
			tools: tools.map((name) => ({ name, version: 'latest' })),
			storageGi
		});
		return { slug };
	}
);

export const removeProject = command(z.object({ slug: z.string() }), async ({ slug }) => {
	const { params } = getRequestEvent();
	requireOrgMember(params.org!, 'admin');
	return deleteProject(params.org!, slug);
});
