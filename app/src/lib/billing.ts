// Shared billing constants. These live outside billing.remote.ts because
// SvelteKit requires every export of a *.remote.ts file to be a remote
// function — plain value exports there fail the production build.

export const RESOURCE_TYPES = [
	'vcpu_hours',
	'mem_gib_hours',
	'net_ingress_internal_bytes',
	'net_egress_internal_bytes',
	'net_ingress_external_bytes',
	'net_egress_external_bytes',
	'block_storage_gib_months',
	'registry_gib_months'
] as const;

export type ResourceType = (typeof RESOURCE_TYPES)[number];

export const COMPONENTS = ['workspace', 'environment', 'zot'] as const;

export type Component = (typeof COMPONENTS)[number];
