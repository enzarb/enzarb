export const config = {
	domain: process.env.PUBLIC_DOMAIN ?? 'enzarb.dev',
	dexIssuer: process.env.OIDC_ISSUER_URL ?? process.env.DEX_ISSUER ?? 'https://auth.enzarb.dev',
	dexClientId: process.env.OIDC_CLIENT_ID ?? process.env.DEX_CLIENT_ID ?? 'enzarb-app',
	dexClientSecret: process.env.OIDC_CLIENT_SECRET ?? process.env.DEX_CLIENT_SECRET ?? '',
	registryUrl: process.env.REGISTRY_URL ?? 'https://registry.enzarb.dev',
	// GitHub OAuth App — empty string means the feature is disabled.
	githubOAuthClientId: process.env.GITHUB_OAUTH_CLIENT_ID ?? '',
	githubOAuthClientSecret: process.env.GITHUB_OAUTH_CLIENT_SECRET ?? ''
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
		// Default only; the effective free-tier limit is the admin-editable
		// `free_max_pvc_gi` platform setting (see settings.ts).
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
