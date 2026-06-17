export const config = {
	domain: process.env.PUBLIC_DOMAIN ?? 'enzarb.dev',
	dexIssuer: process.env.DEX_ISSUER ?? 'https://auth.enzarb.dev',
	dexClientId: process.env.DEX_CLIENT_ID ?? 'enzarb-app',
	dexClientSecret: process.env.DEX_CLIENT_SECRET ?? '',
	giteaUrl: process.env.GITEA_URL ?? 'https://gitea.enzarb.dev',
	registryUrl: process.env.REGISTRY_URL ?? 'https://registry.enzarb.dev'
};

export type TierName = 'free' | 'pro';

export interface TierConfig {
	maxProjects: number;
	maxPvcGi: number;
	cpuLimit: string;
	memoryLimit: string;
	maxEnvironments: number;
	requiresPaymentMethod: boolean;
}

// Loaded from enzarb-config ConfigMap at runtime; defaults used in dev
export const tiers: Record<TierName, TierConfig> = {
	free: {
		maxProjects: 1,
		maxPvcGi: 5,
		cpuLimit: '500m',
		memoryLimit: '512Mi',
		maxEnvironments: 1,
		requiresPaymentMethod: false
	},
	pro: {
		maxProjects: 20,
		maxPvcGi: 100,
		cpuLimit: '16',
		memoryLimit: '32Gi',
		maxEnvironments: 10,
		requiresPaymentMethod: true
	}
};
