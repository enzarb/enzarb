export interface Config {
	databaseUrl: string;
	sessionJwtSecret: string;
	githubOauthClientId: string;
	githubOauthClientSecret: string;
	publicUrl: string;
}

const REQUIRED = {
	databaseUrl: 'DATABASE_URL',
	sessionJwtSecret: 'ENZARB_SESSION_JWT_SECRET',
	githubOauthClientId: 'GITHUB_OAUTH_CLIENT_ID',
	githubOauthClientSecret: 'GITHUB_OAUTH_CLIENT_SECRET',
	publicUrl: 'ENZARB_PUBLIC_URL'
} as const satisfies Record<keyof Config, string>;

/** Parse configuration from an environment map (contracts §9). Reports all problems at once. */
export function parseConfig(source: Record<string, string | undefined>): Config {
	const missing: string[] = [];
	const out = {} as Config;
	for (const [key, name] of Object.entries(REQUIRED) as [keyof Config, string][]) {
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
	return out;
}

let cached: Config | undefined;

/** Lazily parsed so `vite build` and `svelte-check` do not need the environment. */
export function getConfig(): Config {
	return (cached ??= parseConfig(process.env));
}
