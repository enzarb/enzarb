// Shared billing constants. These live outside billing.remote.ts because
// SvelteKit requires every export of a *.remote.ts file to be a remote
// function — plain value exports there fail the production build.

export const RESOURCE_TYPES = [
	'cpu_seconds',
	'mem_gib_seconds',
	'net_ingress_bytes',
	'net_egress_bytes',
	'storage_gib_seconds',
	'gitea_storage_gib_seconds',
	'zot_storage_gib_seconds'
] as const;

export type ResourceType = (typeof RESOURCE_TYPES)[number];

export const COMPONENTS = ['workspace', 'environment', 'gitea', 'zot'] as const;

export type Component = (typeof COMPONENTS)[number];
