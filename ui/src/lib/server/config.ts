import * as v from 'valibot';

export const DEFAULT_GITHUB_APP_SLUG = 'enzarb';

const B64URL = /^[A-Za-z0-9_-]+$/;
const VAPID_SUBJECT = /^(mailto:[^\s@]+@[^\s@]+|https?:\/\/\S+)$/;
const GITHUB_APP_ID = /^\d+$/;
const GITHUB_APP_KEY =
	/^-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]+-----END [A-Z ]*PRIVATE KEY-----$/;

/** Message of the issue a required variable raises when unset; `parseConfig` turns these into one list. */
const MISSING = 'missing';

/** Blank counts as unset: an empty string. */
const trimmed = v.pipe(
	v.optional(v.string(), ''),
	v.transform((s) => s.trim())
);

/** A required variable: unset or blank raises the `MISSING` issue. */
const required = v.pipe(
	trimmed,
	v.rawTransform(({ dataset, addIssue, NEVER }) => {
		if (dataset.value) return dataset.value;
		addIssue({ message: MISSING });
		return NEVER;
	})
);

type Fail = (message: string) => never;

/** An optional variable parsed by `parse`, which reports a problem with its exact message via `fail`. */
function field<T>(parse: (value: string | undefined, fail: Fail) => T) {
	return v.pipe(
		trimmed,
		v.rawTransform(({ dataset, addIssue, NEVER }) => {
			const fail = ((message: string) => {
				addIssue({ message });
				return NEVER;
			}) as Fail;
			return parse(dataset.value || undefined, fail);
		})
	);
}

const isObject = (x: unknown): x is Record<string, unknown> =>
	typeof x === 'object' && x !== null && !Array.isArray(x);

const ComponentVersionsSchema = v.object({
	chart: v.optional(v.string()),
	components: v.record(v.string(), v.string())
});
export type ComponentVersions = v.InferOutput<typeof ComponentVersionsSchema>;

const VapidSchema = v.object({
	publicKey: v.string(),
	privateKey: v.string(),
	/** `mailto:` or https URL identifying the sender to push services. */
	subject: v.string()
});
export type VapidConfig = v.InferOutput<typeof VapidSchema>;

const TracksSchema = v.object({
	/** Base URL of enzarb-tracks, e.g. `http://enzarb-tracks:8080` (no trailing slash). */
	url: v.string(),
	/** Bearer token shared with enzarb-tracks; never sent to the browser. */
	token: v.string()
});
export type TracksConfig = v.InferOutput<typeof TracksSchema>;

/** enzarb-tracks refuses shorter tokens. */
const TRACKS_TOKEN_MIN = 32;

/** The parsed configuration; documents each setting. */
const ConfigOutputSchema = v.object({
	databaseUrl: v.string(),
	sessionJwtSecret: v.string(),
	/** Shared HS256 secret for enzarb-realtime tokens (ENZARB_REALTIME_JWT_SECRET). */
	realtimeJwtSecret: v.string(),
	githubOauthClientId: v.string(),
	githubOauthClientSecret: v.string(),
	publicUrl: v.string(),
	/** Slug of the enzarb GitHub App, used for the install link (ENZARB_GITHUB_APP_SLUG, optional). */
	githubAppSlug: v.string(),
	/** GitHub App id (ENZARB_GITHUB_APP_ID, optional): with the key, authenticates release fetches. */
	githubAppId: v.optional(v.string()),
	/** GitHub App private key PEM (ENZARB_GITHUB_APP_PRIVATE_KEY, optional; `\n`-escaped accepted). */
	githubAppPrivateKey: v.optional(v.string()),
	/**
	 * Also put the realtime token in the websocket URL (ENZARB_REALTIME_LEGACY_TOKEN_URL, optional,
	 * default off). Only for an enzarb-realtime older than enzarb-realtime#8.
	 * TODO: remove once enzarb-realtime#8 is released everywhere.
	 */
	realtimeLegacyTokenUrl: v.boolean(),
	/**
	 * Deployed versions written by the chart (ENZARB_COMPONENT_VERSIONS, optional JSON
	 * `{"chart": "0.4.1", "components": {"enzarb-ui": "v0.9.0"}}`). Empty when unset.
	 */
	componentVersions: ComponentVersionsSchema,
	/**
	 * Web Push (ENZARB_VAPID_PUBLIC_KEY, ENZARB_VAPID_PRIVATE_KEY, ENZARB_VAPID_SUBJECT, all optional).
	 * Absent when no keys are set: notifications are then off. The private key is read only by
	 * `server/push.ts` and never sent to the browser.
	 */
	vapid: v.optional(VapidSchema),
	/**
	 * enzarb-tracks (ENZARB_TRACKS_URL, ENZARB_TRACKS_TOKEN, both optional, set together): validates
	 * and renders `.ez/modes.toml` for the modes designer (ADR-0029). Absent: the designer is off.
	 */
	tracks: v.optional(TracksSchema),
	/** Days of sandbox utilization samples kept (ENZARB_STATS_RETENTION, default 90). */
	sandboxStatsRetentionDays: v.number()
});

/**
 * The environment map in, `Config` out. Entries are in validation order: the first problem (other
 * than missing variables, which are listed together) is the one reported.
 */
export const ConfigSchema = v.pipe(
	v.object({
		DATABASE_URL: required,
		ENZARB_SESSION_JWT_SECRET: required,
		ENZARB_REALTIME_JWT_SECRET: required,
		GITHUB_OAUTH_CLIENT_ID: required,
		GITHUB_OAUTH_CLIENT_SECRET: required,
		ENZARB_PUBLIC_URL: v.pipe(
			required,
			v.rawTransform(({ dataset, addIssue, NEVER }) => {
				let url: URL;
				try {
					url = new URL(dataset.value);
				} catch {
					addIssue({ message: 'ENZARB_PUBLIC_URL must be an absolute URL' });
					return NEVER;
				}
				if (url.protocol !== 'http:' && url.protocol !== 'https:') {
					addIssue({ message: 'ENZARB_PUBLIC_URL must be an http(s) URL' });
					return NEVER;
				}
				return url.origin;
			})
		),
		ENZARB_GITHUB_APP_SLUG: field((value, fail) => {
			const slug = value ?? DEFAULT_GITHUB_APP_SLUG;
			if (!/^[A-Za-z0-9][A-Za-z0-9-]*$/.test(slug)) {
				return fail('ENZARB_GITHUB_APP_SLUG must contain only letters, digits and hyphens');
			}
			return slug;
		}),
		ENZARB_REALTIME_LEGACY_TOKEN_URL: field((value, fail) => {
			const flag = value?.toLowerCase();
			if (flag && flag !== 'true' && flag !== 'false') {
				return fail('ENZARB_REALTIME_LEGACY_TOKEN_URL must be true or false');
			}
			return flag === 'true';
		}),
		ENZARB_STATS_RETENTION: field((value, fail) => {
			const days = value ? Number(value) : 90;
			if (!Number.isInteger(days) || days <= 0) {
				return fail('ENZARB_STATS_RETENTION must be a positive whole number of days');
			}
			return days;
		}),
		ENZARB_GITHUB_APP_ID: trimmed,
		ENZARB_GITHUB_APP_PRIVATE_KEY: v.pipe(
			trimmed,
			v.transform((s) => s.replaceAll('\\n', '\n'))
		),
		ENZARB_COMPONENT_VERSIONS: field((text, fail): ComponentVersions => {
			if (!text) return { components: {} };
			const name = 'ENZARB_COMPONENT_VERSIONS';
			let parsed: unknown;
			try {
				parsed = JSON.parse(text);
			} catch {
				return fail(`${name} must be valid JSON`);
			}
			if (!isObject(parsed)) return fail(`${name} must be a JSON object`);
			const out: ComponentVersions = { components: {} };
			if (parsed.chart !== undefined) {
				if (typeof parsed.chart !== 'string') return fail(`${name}.chart must be a string`);
				out.chart = parsed.chart;
			}
			if (parsed.components !== undefined) {
				if (!isObject(parsed.components)) return fail(`${name}.components must be an object`);
				for (const [key, value] of Object.entries(parsed.components)) {
					if (typeof value !== 'string') return fail(`${name}.components.${key} must be a string`);
					out.components[key] = value;
				}
			}
			return out;
		}),
		ENZARB_VAPID_PUBLIC_KEY: trimmed,
		ENZARB_VAPID_PRIVATE_KEY: trimmed,
		ENZARB_VAPID_SUBJECT: trimmed,
		ENZARB_TRACKS_URL: trimmed,
		ENZARB_TRACKS_TOKEN: trimmed
	}),
	v.rawTransform(({ dataset, addIssue, NEVER }): v.InferInput<typeof ConfigOutputSchema> => {
		const env = dataset.value;
		// A malformed value is treated as unset (the message never carries the value).
		const githubAppId = env.ENZARB_GITHUB_APP_ID;
		const pem = env.ENZARB_GITHUB_APP_PRIVATE_KEY;
		const validId = githubAppId && GITHUB_APP_ID.test(githubAppId) ? githubAppId : undefined;
		const validKey = pem && GITHUB_APP_KEY.test(pem) ? pem : undefined;

		const publicKey = env.ENZARB_VAPID_PUBLIC_KEY;
		const privateKey = env.ENZARB_VAPID_PRIVATE_KEY;
		let vapid: VapidConfig | undefined;
		if (publicKey || privateKey) {
			const fail = (message: string) => {
				addIssue({ message });
				return NEVER;
			};
			if (!publicKey || !privateKey) {
				return fail('ENZARB_VAPID_PUBLIC_KEY and ENZARB_VAPID_PRIVATE_KEY must be set together');
			}
			if (!B64URL.test(publicKey) || !B64URL.test(privateKey)) {
				return fail('ENZARB_VAPID_PUBLIC_KEY and ENZARB_VAPID_PRIVATE_KEY must be base64url');
			}
			const subject = env.ENZARB_VAPID_SUBJECT || env.ENZARB_PUBLIC_URL;
			if (!VAPID_SUBJECT.test(subject)) {
				return fail('ENZARB_VAPID_SUBJECT must be a mailto: or http(s) URL');
			}
			vapid = { publicKey, privateKey, subject };
		}

		let tracks: TracksConfig | undefined;
		if (env.ENZARB_TRACKS_URL || env.ENZARB_TRACKS_TOKEN) {
			const fail = (message: string) => {
				addIssue({ message });
				return NEVER;
			};
			if (!env.ENZARB_TRACKS_URL || !env.ENZARB_TRACKS_TOKEN) {
				return fail('ENZARB_TRACKS_URL and ENZARB_TRACKS_TOKEN must be set together');
			}
			let url: URL;
			try {
				url = new URL(env.ENZARB_TRACKS_URL);
			} catch {
				return fail('ENZARB_TRACKS_URL must be an absolute URL');
			}
			if (url.protocol !== 'http:' && url.protocol !== 'https:') {
				return fail('ENZARB_TRACKS_URL must be an http(s) URL');
			}
			if (env.ENZARB_TRACKS_TOKEN.length < TRACKS_TOKEN_MIN) {
				return fail(`ENZARB_TRACKS_TOKEN must be at least ${TRACKS_TOKEN_MIN} characters`);
			}
			tracks = { url: url.href.replace(/\/+$/, ''), token: env.ENZARB_TRACKS_TOKEN };
		}

		return {
			databaseUrl: env.DATABASE_URL,
			sessionJwtSecret: env.ENZARB_SESSION_JWT_SECRET,
			realtimeJwtSecret: env.ENZARB_REALTIME_JWT_SECRET,
			githubOauthClientId: env.GITHUB_OAUTH_CLIENT_ID,
			githubOauthClientSecret: env.GITHUB_OAUTH_CLIENT_SECRET,
			publicUrl: env.ENZARB_PUBLIC_URL,
			githubAppSlug: env.ENZARB_GITHUB_APP_SLUG,
			...(validId && validKey ? { githubAppId: validId, githubAppPrivateKey: validKey } : {}),
			realtimeLegacyTokenUrl: env.ENZARB_REALTIME_LEGACY_TOKEN_URL,
			componentVersions: env.ENZARB_COMPONENT_VERSIONS,
			...(vapid ? { vapid } : {}),
			...(tracks ? { tracks } : {}),
			sandboxStatsRetentionDays: env.ENZARB_STATS_RETENTION
		};
	}),
	ConfigOutputSchema
);

export type Config = v.InferOutput<typeof ConfigSchema>;

/** Parse configuration from an environment map (contracts §9). Missing variables are listed together. */
export function parseConfig(source: Record<string, string | undefined>): Config {
	const result = v.safeParse(ConfigSchema, source);
	if (result.success) return result.output;
	const missing = result.issues
		.filter((issue) => issue.message === MISSING)
		.map((issue) => String(issue.path?.[0]?.key));
	if (missing.length)
		throw new Error(`Missing required environment variables: ${missing.join(', ')}`);
	throw new Error(result.issues[0].message);
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
