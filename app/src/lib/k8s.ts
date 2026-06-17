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
				gatewayRef: { name: 'enzarb-gateway', namespace: 'enzarb-system' }
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
	}, undefined, undefined, undefined, undefined, {
		headers: { 'Content-Type': 'application/json-patch+json' }
	});
}
