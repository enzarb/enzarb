import * as k8s from '@kubernetes/client-node';

const kc = new k8s.KubeConfig();

if (process.env.KUBECONFIG || process.env.NODE_ENV === 'development') {
	kc.loadFromDefault();
} else {
	kc.loadFromCluster();
}

export const coreApi = kc.makeApiClient(k8s.CoreV1Api);
export const appsApi = kc.makeApiClient(k8s.AppsV1Api);
export const customApi = kc.makeApiClient(k8s.CustomObjectsApi);

const GROUP = 'enzarb.io';
const VERSION = 'v1alpha1';

export function orgNamespace(orgId: string) {
	return `user-${orgId}`;
}

// The cluster-scoped Organization CR owns the org's namespace; the operator
// reconciles it and provisions `user-<orgId>`. The app creates the CR (named by
// the immutable orgId) but never touches namespaces directly.
export async function createOrganization(orgId: string, slug: string, displayName: string) {
	try {
		await customApi.createClusterCustomObject({
			group: GROUP,
			version: VERSION,
			plural: 'organizations',
			body: {
				apiVersion: `${GROUP}/${VERSION}`,
				kind: 'Organization',
				metadata: { name: orgId },
				spec: { orgId, slug, displayName }
			}
		});
	} catch (err) {
		// 409 = already exists; idempotent across replicas / retries.
		if ((err as { code?: number }).code !== 409) throw err;
	}
}

export async function getOrganization(orgId: string) {
	try {
		return (await customApi.getClusterCustomObject({
			group: GROUP,
			version: VERSION,
			plural: 'organizations',
			name: orgId
		})) as { status?: { phase?: string } };
	} catch (err) {
		if ((err as { code?: number }).code === 404) return null;
		throw err;
	}
}

export async function isOrgReady(orgId: string) {
	const org = await getOrganization(orgId);
	return org?.status?.phase === 'Ready';
}

// Polls the Organization CR until the operator marks it Ready (namespace
// provisioned), or the timeout elapses. Returns whether it became ready.
export async function waitForOrganizationReady(orgId: string, timeoutMs = 30_000) {
	const deadline = Date.now() + timeoutMs;
	for (;;) {
		if (await isOrgReady(orgId)) return true;
		if (Date.now() >= deadline) return false;
		await new Promise((r) => setTimeout(r, 1000));
	}
}

export async function listProjects(orgId: string) {
	const ns = orgNamespace(orgId);
	const res = await customApi.listNamespacedCustomObject({
		group: GROUP,
		version: VERSION,
		namespace: ns,
		plural: 'projects'
	});
	return (res as any).items ?? [];
}

export async function getProject(orgId: string, slug: string) {
	const ns = orgNamespace(orgId);
	return customApi.getNamespacedCustomObject({
		group: GROUP,
		version: VERSION,
		namespace: ns,
		plural: 'projects',
		name: slug
	});
}

export async function createProject(orgId: string, spec: {
	slug: string;
	displayName: string;
	tools: { name: string; version: string }[];
	storageGi: number;
}) {
	const ns = orgNamespace(orgId);
	return customApi.createNamespacedCustomObject({
		group: GROUP,
		version: VERSION,
		namespace: ns,
		plural: 'projects',
		body: {
			apiVersion: `${GROUP}/${VERSION}`,
			kind: 'Project',
			metadata: { name: spec.slug, namespace: ns },
			spec: {
				orgId,
				slug: spec.slug,
				displayName: spec.displayName,
				tools: spec.tools,
				storage: { size: `${spec.storageGi}Gi` }
			}
		}
	});
}

// Soft-delete: the operator retains the workspace (scaled to zero, data kept)
// and hard-deletes it once `enzarb.io/purge-after` passes. Recovery clears it.
const PURGE_ANNOTATION = 'enzarb.io/purge-after';
const PURGE_POINTER = '/metadata/annotations/enzarb.io~1purge-after';

export function purgeAfterOf(obj: {
	metadata?: { annotations?: Record<string, string> };
}): string | null {
	return obj.metadata?.annotations?.[PURGE_ANNOTATION] ?? null;
}

function purgeTimestamp(retentionDays: number): string {
	return new Date(Date.now() + retentionDays * 86_400_000).toISOString();
}

// JSON Patch (the client's default patch media type) to set the purge
// annotation, handling an object with no annotations map yet.
function setPurgePatch(obj: { metadata?: { annotations?: unknown } }, ts: string) {
	return obj.metadata?.annotations
		? [{ op: 'add', path: PURGE_POINTER, value: ts }]
		: [{ op: 'add', path: '/metadata/annotations', value: { [PURGE_ANNOTATION]: ts } }];
}

export async function softDeleteProject(orgId: string, slug: string, retentionDays: number) {
	const ns = orgNamespace(orgId);
	const proj = (await getProject(orgId, slug)) as { metadata?: { annotations?: unknown } };
	const ts = purgeTimestamp(retentionDays);
	await customApi.patchNamespacedCustomObject({
		group: GROUP,
		version: VERSION,
		namespace: ns,
		plural: 'projects',
		name: slug,
		body: setPurgePatch(proj, ts)
	});
	return ts;
}

export async function recoverProject(orgId: string, slug: string) {
	const ns = orgNamespace(orgId);
	const proj = (await getProject(orgId, slug)) as {
		metadata?: { annotations?: Record<string, string> };
	};
	if (!purgeAfterOf(proj)) return;
	await customApi.patchNamespacedCustomObject({
		group: GROUP,
		version: VERSION,
		namespace: ns,
		plural: 'projects',
		name: slug,
		body: [{ op: 'remove', path: PURGE_POINTER }]
	});
}

export async function deleteProject(orgId: string, slug: string) {
	const ns = orgNamespace(orgId);
	return customApi.deleteNamespacedCustomObject({
		group: GROUP,
		version: VERSION,
		namespace: ns,
		plural: 'projects',
		name: slug
	});
}

// Soft-delete an org: stamp the Organization CR and cascade to every Project in
// its namespace so all workspaces scale down and purge together.
export async function softDeleteOrganization(orgId: string, retentionDays: number) {
	const ts = purgeTimestamp(retentionDays);
	const org = (await getOrganization(orgId)) as { metadata?: { annotations?: unknown } } | null;
	if (org) {
		await customApi.patchClusterCustomObject({
			group: GROUP,
			version: VERSION,
			plural: 'organizations',
			name: orgId,
			body: setPurgePatch(org, ts)
		});
	}
	const projects = await listProjects(orgId);
	for (const p of projects as { metadata?: { name?: string; annotations?: Record<string, string> } }[]) {
		const name = p.metadata?.name;
		if (!name || purgeAfterOf(p)) continue;
		await customApi.patchNamespacedCustomObject({
			group: GROUP,
			version: VERSION,
			namespace: orgNamespace(orgId),
			plural: 'projects',
			name,
			body: setPurgePatch(p, ts)
		});
	}
	return ts;
}

export async function recoverOrganization(orgId: string) {
	const org = (await getOrganization(orgId)) as {
		metadata?: { annotations?: Record<string, string> };
	} | null;
	if (org && purgeAfterOf(org)) {
		await customApi.patchClusterCustomObject({
			group: GROUP,
			version: VERSION,
			plural: 'organizations',
			name: orgId,
			body: [{ op: 'remove', path: PURGE_POINTER }]
		});
	}
	const projects = await listProjects(orgId);
	for (const p of projects as { metadata?: { name?: string; annotations?: Record<string, string> } }[]) {
		const name = p.metadata?.name;
		if (!name || !purgeAfterOf(p)) continue;
		await customApi.patchNamespacedCustomObject({
			group: GROUP,
			version: VERSION,
			namespace: orgNamespace(orgId),
			plural: 'projects',
			name,
			body: [{ op: 'remove', path: PURGE_POINTER }]
		});
	}
}

export async function listEnvironments(orgId: string, projectSlug: string) {
	const ns = orgNamespace(orgId);
	const res = await customApi.listNamespacedCustomObject({
		group: GROUP,
		version: VERSION,
		namespace: ns,
		plural: 'environments'
	});
	const items = (res as any).items ?? [];
	return items.filter((e: any) => e.spec?.projectRef?.name === projectSlug);
}

export async function createEnvironment(orgId: string, projectSlug: string, slug: string) {
	const ns = orgNamespace(orgId);
	return customApi.createNamespacedCustomObject({
		group: GROUP,
		version: VERSION,
		namespace: ns,
		plural: 'environments',
		body: {
			apiVersion: `${GROUP}/${VERSION}`,
			kind: 'Environment',
			metadata: { name: `${projectSlug}-${slug}`, namespace: ns },
			spec: {
				projectRef: { name: projectSlug },
				slug,
				gatewayRef: { name: 'enzarb', namespace: 'enzarb-system' }
			}
		}
	});
}

export async function addCustomDomain(orgId: string, envName: string, fqdn: string) {
	const ns = orgNamespace(orgId);
	const env = await customApi.getNamespacedCustomObject({
		group: GROUP,
		version: VERSION,
		namespace: ns,
		plural: 'environments',
		name: envName
	}) as any;

	const domains = env.spec?.customDomains ?? [];
	domains.push({ fqdn, tlsMode: 'acme' });

	return customApi.patchNamespacedCustomObject({
		group: GROUP,
		version: VERSION,
		namespace: ns,
		plural: 'environments',
		name: envName,
		body: [{ op: 'replace', path: '/spec/customDomains', value: domains }]
	});
}
