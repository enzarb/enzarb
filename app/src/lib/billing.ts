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

export const RESOURCE_LABELS: Record<string, string> = {
	vcpu_hours: 'CPU',
	mem_gib_hours: 'Memory',
	net_ingress_internal_bytes: 'Net In (internal)',
	net_egress_internal_bytes: 'Net Out (internal)',
	net_ingress_external_bytes: 'Net In (external)',
	net_egress_external_bytes: 'Net Out (external)',
	block_storage_gib_months: 'Block Storage',
	registry_gib_months: 'Registry Storage'
};

export const RESOURCE_COLORS: Record<string, string> = {
	vcpu_hours: '#58a6ff',
	mem_gib_hours: '#3fb950',
	net_ingress_internal_bytes: '#d29922',
	net_egress_internal_bytes: '#e3b341',
	net_ingress_external_bytes: '#db6d28',
	net_egress_external_bytes: '#f0883e',
	block_storage_gib_months: '#a371f7',
	registry_gib_months: '#56d4dd'
};

export const usd = (n: number) =>
	n.toLocaleString('en-US', {
		style: 'currency',
		currency: 'USD',
		minimumFractionDigits: 2,
		maximumFractionDigits: n < 1 ? 4 : 2
	});

export const fmtVCPUHours = (h: number) =>
	h.toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' vCPU-hr';

export const fmtGiBHours = (h: number) =>
	h.toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' GiB-hr';

export const fmtGiBMonths = (m: number) =>
	m.toLocaleString('en-US', { maximumFractionDigits: 3 }) + ' GiB-mo';

export const fmtBytes = (bytes: number) => {
	if (bytes === 0) return '—';
	const units = ['B', 'KB', 'MB', 'GB', 'TB'];
	const i = Math.min(Math.floor(Math.log2(bytes) / 10), units.length - 1);
	return (bytes / Math.pow(1024, i)).toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' ' + units[i];
};

// Raw per-metric formatter (no unit label) for compact chart axes.
export const fmtRaw = (resourceType: string, n: number): string => {
	switch (resourceType) {
		case 'vcpu_hours':
			return n.toLocaleString('en-US', { maximumFractionDigits: 4 });
		case 'mem_gib_hours':
			return n.toLocaleString('en-US', { maximumFractionDigits: 4 });
		case 'block_storage_gib_months':
		case 'registry_gib_months':
			return n.toLocaleString('en-US', { maximumFractionDigits: 4 });
		default:
			return fmtBytes(n);
	}
};
