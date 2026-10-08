export interface Config {
	databaseUrl: string;
	sessionJwtSecret: string;
	githubOauthClientId: string;
	githubOauthClientSecret: string;
	publicUrl: string;
	/** RuntimeClass names offered for Docker-enabled sandboxes (ADR-0012). */
	sandboxRuntimeClasses: string[];
}

const REQUIRED = {
	databaseUrl: 'DATABASE_URL',
	sessionJwtSecret: 'ENZARB_SESSION_JWT_SECRET',
	githubOauthClientId: 'GITHUB_OAUTH_CLIENT_ID',
	githubOauthClientSecret: 'GITHUB_OAUTH_CLIENT_SECRET',
	publicUrl: 'ENZARB_PUBLIC_URL'
} as const satisfies Record<Exclude<keyof Config, 'sandboxRuntimeClasses'>, string>;

export const DEFAULT_RUNTIME_CLASSES = ['gvisor', 'kata'];

/** Comma-separated list; blank or unset falls back to the default. */
export function parseRuntimeClasses(raw: string | undefined): string[] {
	const list = [
		...new Set(
			(raw ?? '')
				.split(',')
				.map((s) => s.trim())
				.filter(Boolean)
		)
	];
	return list.length ? list : [...DEFAULT_RUNTIME_CLASSES];
}

/** Parse configuration from an environment map (contracts §9). Reports all problems at once. */
export function parseConfig(source: Record<string, string | undefined>): Config {
	const missing: string[] = [];
	const out = {} as Record<Exclude<keyof Config, 'sandboxRuntimeClasses'>, string>;
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
	return {
		...out,
		sandboxRuntimeClasses: parseRuntimeClasses(source.ENZARB_SANDBOX_RUNTIME_CLASSES)
	};
}

let cached: Config | undefined;

/** Lazily parsed so `vite build` and `svelte-check` do not need the environment. */
export function getConfig(): Config {
	return (cached ??= parseConfig(process.env));
}
