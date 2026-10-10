export interface Config {
	databaseUrl: string;
	sessionJwtSecret: string;
	githubOauthClientId: string;
	githubOauthClientSecret: string;
	publicUrl: string;
	/** Slug of the enzarb GitHub App, used for the install link (ENZARB_GITHUB_APP_SLUG, optional). */
	githubAppSlug: string;
}

const REQUIRED: Record<Exclude<keyof Config, 'githubAppSlug'>, string> = {
	databaseUrl: 'DATABASE_URL',
	sessionJwtSecret: 'ENZARB_SESSION_JWT_SECRET',
	githubOauthClientId: 'GITHUB_OAUTH_CLIENT_ID',
	githubOauthClientSecret: 'GITHUB_OAUTH_CLIENT_SECRET',
	publicUrl: 'ENZARB_PUBLIC_URL'
};

export const DEFAULT_GITHUB_APP_SLUG = 'enzarb';

/** Parse configuration from an environment map (contracts §9). Reports all problems at once. */
export function parseConfig(source: Record<string, string | undefined>): Config {
	const missing: string[] = [];
	const out = {} as Record<Exclude<keyof Config, 'githubAppSlug'>, string>;
	for (const [key, name] of Object.entries(REQUIRED) as [keyof typeof out, string][]) {
		const value = source[name]?.trim();
		if (value) out[key] = value;
		else missing.push(name);
	}
	if (missing.length)
		throw new Error(`Missing required environment variables: ${missing.join(', ')}`);

	let url: URL;
	try {
		url = new URL(out.publicUrl);
	} catch {
		throw new Error('ENZARB_PUBLIC_URL must be an absolute URL');
	}
	if (url.protocol !== 'http:' && url.protocol !== 'https:') {
		throw new Error('ENZARB_PUBLIC_URL must be an http(s) URL');
	}
	out.publicUrl = url.origin;

	const githubAppSlug = source.ENZARB_GITHUB_APP_SLUG?.trim() || DEFAULT_GITHUB_APP_SLUG;
	if (!/^[A-Za-z0-9][A-Za-z0-9-]*$/.test(githubAppSlug)) {
		throw new Error('ENZARB_GITHUB_APP_SLUG must contain only letters, digits and hyphens');
	}
	return { ...out, githubAppSlug };
}

let cached: Config | undefined;

/** Lazily parsed so `vite build` and `svelte-check` do not need the environment. */
export function getConfig(): Config {
	return (cached ??= parseConfig(process.env));
}
