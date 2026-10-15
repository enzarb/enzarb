import { NEEDS_YOU_STALE_HOURS_DEFAULT } from '#lib/work-items.ts';

export interface Config {
	databaseUrl: string;
	sessionJwtSecret: string;
	/** Shared HS256 secret for enzarb-realtime tokens (ENZARB_REALTIME_JWT_SECRET). */
	realtimeJwtSecret: string;
	githubOauthClientId: string;
	githubOauthClientSecret: string;
	publicUrl: string;
	/** Slug of the enzarb GitHub App, used for the install link (ENZARB_GITHUB_APP_SLUG, optional). */
	githubAppSlug: string;
	/** GitHub App id (ENZARB_GITHUB_APP_ID, optional): with the key, authenticates release fetches. */
	githubAppId?: string;
	/** GitHub App private key PEM (ENZARB_GITHUB_APP_PRIVATE_KEY, optional; `\n`-escaped accepted). */
	githubAppPrivateKey?: string;
	/**
	 * Also put the realtime token in the websocket URL (ENZARB_REALTIME_LEGACY_TOKEN_URL, optional,
	 * default off). Only for an enzarb-realtime older than enzarb-realtime#8.
	 * TODO: remove once enzarb-realtime#8 is released everywhere.
	 */
	realtimeLegacyTokenUrl: boolean;
	/**
	 * Deployed versions written by the chart (ENZARB_COMPONENT_VERSIONS, optional JSON
	 * `{"chart": "0.4.1", "components": {"enzarb-ui": "v0.9.0"}}`). Empty when unset.
	 */
	componentVersions: ComponentVersions;
	/** Hours a blocked change request may sit idle before it joins "Needs you" (NEEDS_YOU_STALE_HOURS, default 24). */
	needsYouStaleHours: number;
	/**
	 * Web Push (ENZARB_VAPID_PUBLIC_KEY, ENZARB_VAPID_PRIVATE_KEY, ENZARB_VAPID_SUBJECT, all optional).
	 * Absent when no keys are set: notifications are then off. The private key is read only by
	 * `server/push.ts` and never sent to the browser.
	 */
	vapid?: VapidConfig;
	/** Days of sandbox utilization samples kept (ENZARB_STATS_RETENTION, default 90; the workflows poller reads the same setting). */
	sandboxStatsRetentionDays: number;
}

export interface VapidConfig {
	publicKey: string;
	privateKey: string;
	/** `mailto:` or https URL identifying the sender to push services. */
	subject: string;
}

export interface ComponentVersions {
	chart?: string;
	components: Record<string, string>;
}

type RequiredKey = Exclude<
	keyof Config,
	| 'githubAppSlug'
	| 'githubAppId'
	| 'githubAppPrivateKey'
	| 'realtimeLegacyTokenUrl'
	| 'componentVersions'
	| 'needsYouStaleHours'
	| 'vapid'
	| 'sandboxStatsRetentionDays'
>;

function parseComponentVersions(raw: string | undefined): ComponentVersions {
	const text = raw?.trim();
	if (!text) return { components: {} };
	const name = 'ENZARB_COMPONENT_VERSIONS';
	let parsed: unknown;
	try {
		parsed = JSON.parse(text);
	} catch {
		throw new Error(`${name} must be valid JSON`);
	}
	const isObject = (v: unknown): v is Record<string, unknown> =>
		typeof v === 'object' && v !== null && !Array.isArray(v);
	if (!isObject(parsed)) throw new Error(`${name} must be a JSON object`);
	const out: ComponentVersions = { components: {} };
	if (parsed.chart !== undefined) {
		if (typeof parsed.chart !== 'string') throw new Error(`${name}.chart must be a string`);
		out.chart = parsed.chart;
	}
	if (parsed.components !== undefined) {
		if (!isObject(parsed.components)) throw new Error(`${name}.components must be an object`);
		for (const [key, value] of Object.entries(parsed.components)) {
			if (typeof value !== 'string') throw new Error(`${name}.components.${key} must be a string`);
			out.components[key] = value;
		}
	}
	return out;
}

const B64URL = /^[A-Za-z0-9_-]+$/;

function parseVapid(source: Record<string, string | undefined>, publicUrl: string) {
	const publicKey = source.ENZARB_VAPID_PUBLIC_KEY?.trim();
	const privateKey = source.ENZARB_VAPID_PRIVATE_KEY?.trim();
	if (!publicKey && !privateKey) return undefined;
	if (!publicKey || !privateKey) {
		throw new Error('ENZARB_VAPID_PUBLIC_KEY and ENZARB_VAPID_PRIVATE_KEY must be set together');
	}
	if (!B64URL.test(publicKey) || !B64URL.test(privateKey)) {
		throw new Error('ENZARB_VAPID_PUBLIC_KEY and ENZARB_VAPID_PRIVATE_KEY must be base64url');
	}
	const subject = source.ENZARB_VAPID_SUBJECT?.trim() || publicUrl;
	if (!/^(mailto:[^\s@]+@[^\s@]+|https?:\/\/\S+)$/.test(subject)) {
		throw new Error('ENZARB_VAPID_SUBJECT must be a mailto: or http(s) URL');
	}
	return { publicKey, privateKey, subject };
}

const REQUIRED: Record<RequiredKey, string> = {
	databaseUrl: 'DATABASE_URL',
	sessionJwtSecret: 'ENZARB_SESSION_JWT_SECRET',
	realtimeJwtSecret: 'ENZARB_REALTIME_JWT_SECRET',
	githubOauthClientId: 'GITHUB_OAUTH_CLIENT_ID',
	githubOauthClientSecret: 'GITHUB_OAUTH_CLIENT_SECRET',
	publicUrl: 'ENZARB_PUBLIC_URL'
};

export const DEFAULT_GITHUB_APP_SLUG = 'enzarb';

/** Parse configuration from an environment map (contracts §9). Reports all problems at once. */
export function parseConfig(source: Record<string, string | undefined>): Config {
	const missing: string[] = [];
	const out = {} as Record<RequiredKey, string>;
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
	const legacy = source.ENZARB_REALTIME_LEGACY_TOKEN_URL?.trim().toLowerCase();
	if (legacy && legacy !== 'true' && legacy !== 'false') {
		throw new Error('ENZARB_REALTIME_LEGACY_TOKEN_URL must be true or false');
	}
	const staleRaw = source.NEEDS_YOU_STALE_HOURS?.trim();
	const needsYouStaleHours = staleRaw ? Number(staleRaw) : NEEDS_YOU_STALE_HOURS_DEFAULT;
	if (!Number.isFinite(needsYouStaleHours) || needsYouStaleHours <= 0) {
		throw new Error('NEEDS_YOU_STALE_HOURS must be a positive number');
	}
	const retentionRaw = source.ENZARB_STATS_RETENTION?.trim();
	const sandboxStatsRetentionDays = retentionRaw ? Number(retentionRaw) : 90;
	if (!Number.isInteger(sandboxStatsRetentionDays) || sandboxStatsRetentionDays <= 0) {
		throw new Error('ENZARB_STATS_RETENTION must be a positive whole number of days');
	}
	const githubAppId = source.ENZARB_GITHUB_APP_ID?.trim();
	const pem = source.ENZARB_GITHUB_APP_PRIVATE_KEY?.trim().replaceAll('\\n', '\n');
	// A malformed value is treated as unset (the message never carries the value).
	const validId = githubAppId && /^\d+$/.test(githubAppId) ? githubAppId : undefined;
	const validKey =
		pem && /^-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]+-----END [A-Z ]*PRIVATE KEY-----$/.test(pem)
			? pem
			: undefined;
	return {
		...out,
		needsYouStaleHours,
		sandboxStatsRetentionDays,
		...(validId && validKey ? { githubAppId: validId, githubAppPrivateKey: validKey } : {}),
		githubAppSlug,
		realtimeLegacyTokenUrl: legacy === 'true',
		componentVersions: parseComponentVersions(source.ENZARB_COMPONENT_VERSIONS),
		vapid: parseVapid(source, out.publicUrl)
	};
}

let cached: Config | undefined;

/** Lazily parsed so `vite build` and `svelte-check` do not need the environment. */
export function getConfig(): Config {
	if (cached) return cached;
	cached = parseConfig(process.env);
	if (cached.realtimeLegacyTokenUrl) {
		console.warn(
			'ENZARB_REALTIME_LEGACY_TOKEN_URL is on: the realtime token is also put in the websocket URL'
		);
	}
	return cached;
}
