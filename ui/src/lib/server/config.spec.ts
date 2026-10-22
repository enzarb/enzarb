import { describe, expect, expectTypeOf, it } from 'vitest';

import { parseConfig, type Config, type VapidConfig } from './config.ts';

const valid = {
	DATABASE_URL: 'postgres://u:p@h:5432/enzarb',
	ENZARB_SESSION_JWT_SECRET: 's3cret',
	ENZARB_REALTIME_JWT_SECRET: 'rt-s3cret',
	GITHUB_OAUTH_CLIENT_ID: 'id',
	GITHUB_OAUTH_CLIENT_SECRET: 'secret',
	ENZARB_PUBLIC_URL: 'https://enzarb.example.com/'
};

describe('parseConfig', () => {
	it('parses a complete environment and normalizes the public URL', () => {
		expect(parseConfig(valid)).toEqual({
			databaseUrl: valid.DATABASE_URL,
			sessionJwtSecret: 's3cret',
			realtimeJwtSecret: 'rt-s3cret',
			githubOauthClientId: 'id',
			githubOauthClientSecret: 'secret',
			publicUrl: 'https://enzarb.example.com',
			githubAppSlug: 'enzarb',
			realtimeLegacyTokenUrl: false,
			componentVersions: { components: {} },
			sandboxStatsRetentionDays: 90
		});
	});

	it('reads ENZARB_STATS_RETENTION and rejects non-positive or fractional values', () => {
		expect(parseConfig({ ...valid, ENZARB_STATS_RETENTION: '14' }).sandboxStatsRetentionDays).toBe(
			14
		);
		for (const bad of ['0', '-1', '1.5', 'abc'])
			expect(() => parseConfig({ ...valid, ENZARB_STATS_RETENTION: bad })).toThrow(
				/ENZARB_STATS_RETENTION/
			);
	});

	describe('GitHub App credentials', () => {
		const pem = '-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----';
		it('are optional and unset by default', () => {
			const c = parseConfig(valid);
			expect(c.githubAppId).toBeUndefined();
			expect(c.githubAppPrivateKey).toBeUndefined();
		});
		it('accept an \\n-escaped PEM', () => {
			const c = parseConfig({
				...valid,
				ENZARB_GITHUB_APP_ID: ' 123 ',
				ENZARB_GITHUB_APP_PRIVATE_KEY: pem.replaceAll('\n', '\\n')
			});
			expect(c.githubAppId).toBe('123');
			expect(c.githubAppPrivateKey).toBe(pem);
		});
		it('treat a malformed value as unset without throwing', () => {
			const c = parseConfig({
				...valid,
				ENZARB_GITHUB_APP_ID: '123',
				ENZARB_GITHUB_APP_PRIVATE_KEY: 'garbage'
			});
			expect(c.githubAppId).toBeUndefined();
			expect(c.githubAppPrivateKey).toBeUndefined();
		});
	});

	describe('ENZARB_COMPONENT_VERSIONS', () => {
		const parse = (v?: string) =>
			parseConfig({ ...valid, ENZARB_COMPONENT_VERSIONS: v }).componentVersions;

		it('is an empty map when absent or blank', () => {
			expect(parse(undefined)).toEqual({ components: {} });
			expect(parse('  ')).toEqual({ components: {} });
		});
		it('parses valid JSON', () => {
			expect(parse('{"chart":"0.4.1","components":{"enzarb-ui":"v0.9.0"}}')).toEqual({
				chart: '0.4.1',
				components: { 'enzarb-ui': 'v0.9.0' }
			});
			expect(parse('{}')).toEqual({ components: {} });
		});
		it('rejects invalid JSON, non-objects and non-string tags', () => {
			expect(() => parse('{nope')).toThrow(/valid JSON/);
			expect(() => parse('[]')).toThrow(/JSON object/);
			expect(() => parse('"x"')).toThrow(/JSON object/);
			expect(() => parse('{"components":[]}')).toThrow(/components must be an object/);
			expect(() => parse('{"components":{"enzarb-ui":1}}')).toThrow(/enzarb-ui must be a string/);
			expect(() => parse('{"chart":2}')).toThrow(/chart must be a string/);
		});
	});

	it('parses the legacy realtime token URL flag', () => {
		const flag = (v?: string) =>
			parseConfig({ ...valid, ENZARB_REALTIME_LEGACY_TOKEN_URL: v }).realtimeLegacyTokenUrl;
		expect(flag()).toBe(false);
		expect(flag('true')).toBe(true);
		expect(flag('false')).toBe(false);
		expect(() => flag('yes')).toThrow(/ENZARB_REALTIME_LEGACY_TOKEN_URL/);
	});

	it('reads the optional GitHub App slug', () => {
		expect(parseConfig({ ...valid, ENZARB_GITHUB_APP_SLUG: ' my-app ' }).githubAppSlug).toBe(
			'my-app'
		);
		expect(parseConfig({ ...valid, ENZARB_GITHUB_APP_SLUG: '  ' }).githubAppSlug).toBe('enzarb');
		expect(() => parseConfig({ ...valid, ENZARB_GITHUB_APP_SLUG: 'a/b' })).toThrow(
			/ENZARB_GITHUB_APP_SLUG/
		);
	});

	describe('tracks', () => {
		const token = 't'.repeat(32);
		const parse = (extra: Record<string, string>) => parseConfig({ ...valid, ...extra }).tracks;

		it('is absent when neither variable is set', () => {
			expect(parse({})).toBeUndefined();
		});

		it('reads the URL (without a trailing slash) and the token', () => {
			expect(
				parse({ ENZARB_TRACKS_URL: 'http://enzarb-tracks:8080/', ENZARB_TRACKS_TOKEN: token })
			).toEqual({ url: 'http://enzarb-tracks:8080', token });
		});

		it('rejects half a configuration, bad URLs and short tokens without echoing the token', () => {
			expect(() => parse({ ENZARB_TRACKS_URL: 'http://t' })).toThrow(/set together/);
			expect(() => parse({ ENZARB_TRACKS_TOKEN: token })).toThrow(/set together/);
			expect(() => parse({ ENZARB_TRACKS_URL: 'tracks', ENZARB_TRACKS_TOKEN: token })).toThrow(
				/absolute URL/
			);
			expect(() => parse({ ENZARB_TRACKS_URL: 'ftp://t', ENZARB_TRACKS_TOKEN: token })).toThrow(
				/http\(s\)/
			);
			const err = (() => {
				try {
					parse({ ENZARB_TRACKS_URL: 'http://t', ENZARB_TRACKS_TOKEN: 'short-secret' });
				} catch (e) {
					return (e as Error).message;
				}
			})();
			expect(err).toMatch(/at least 32/);
			expect(err).not.toContain('short-secret');
		});
	});

	describe('VAPID keys', () => {
		const keys = { ENZARB_VAPID_PUBLIC_KEY: 'BPub_-1', ENZARB_VAPID_PRIVATE_KEY: 'priv_-2' };
		const parse = (extra: Record<string, string>) => parseConfig({ ...valid, ...extra }).vapid;

		it('is absent without keys and defaults the subject to the public origin', () => {
			expect(parse({})).toBeUndefined();
			expect(parse(keys)).toEqual({
				publicKey: 'BPub_-1',
				privateKey: 'priv_-2',
				subject: 'https://enzarb.example.com'
			});
		});

		it('takes a mailto subject', () => {
			expect(parse({ ...keys, ENZARB_VAPID_SUBJECT: 'mailto:ops@example.com' })?.subject).toBe(
				'mailto:ops@example.com'
			);
		});

		it('rejects half a keypair, non-base64url keys and a bad subject', () => {
			expect(() => parse({ ENZARB_VAPID_PUBLIC_KEY: 'x' })).toThrow(/set together/);
			expect(() => parse({ ENZARB_VAPID_PRIVATE_KEY: 'x' })).toThrow(/set together/);
			expect(() => parse({ ...keys, ENZARB_VAPID_PRIVATE_KEY: 'a+b/c=' })).toThrow(/base64url/);
			expect(() => parse({ ...keys, ENZARB_VAPID_SUBJECT: 'ops' })).toThrow(/ENZARB_VAPID_SUBJECT/);
		});
	});

	it('lists every missing variable at once', () => {
		expect(() => parseConfig({ DATABASE_URL: valid.DATABASE_URL })).toThrow(
			/ENZARB_SESSION_JWT_SECRET, ENZARB_REALTIME_JWT_SECRET, GITHUB_OAUTH_CLIENT_ID, GITHUB_OAUTH_CLIENT_SECRET, ENZARB_PUBLIC_URL/
		);
	});

	it('reports several missing variables in schema order, ignoring input order', () => {
		expect(() =>
			parseConfig({
				ENZARB_PUBLIC_URL: 'not a url',
				GITHUB_OAUTH_CLIENT_ID: valid.GITHUB_OAUTH_CLIENT_ID,
				ENZARB_SESSION_JWT_SECRET: valid.ENZARB_SESSION_JWT_SECRET
			})
		).toThrow(
			/^Missing required environment variables: DATABASE_URL, ENZARB_REALTIME_JWT_SECRET, GITHUB_OAUTH_CLIENT_SECRET$/
		);
	});

	it('reports missing variables before invalid values', () => {
		expect(() =>
			parseConfig({ ...valid, DATABASE_URL: undefined, ENZARB_STATS_RETENTION: 'x' })
		).toThrow(/^Missing required environment variables: DATABASE_URL$/);
	});

	it('drops a malformed GitHub App key without echoing it, and needs both halves', () => {
		const secret = 'super-secret-garbage';
		const c = parseConfig({
			...valid,
			ENZARB_GITHUB_APP_ID: '123',
			ENZARB_GITHUB_APP_PRIVATE_KEY: secret
		});
		expect(JSON.stringify(c)).not.toContain(secret);
		expect(parseConfig({ ...valid, ENZARB_GITHUB_APP_ID: '123' }).githubAppId).toBeUndefined();
	});

	it('accepts the legacy flag case-insensitively and defaults blank optionals', () => {
		const c = parseConfig({
			...valid,
			ENZARB_REALTIME_LEGACY_TOKEN_URL: 'TRUE',
			ENZARB_STATS_RETENTION: ' '
		});
		expect(c.realtimeLegacyTokenUrl).toBe(true);
		expect(c.sandboxStatsRetentionDays).toBe(90);
		expect(
			parseConfig({ ...valid, ENZARB_REALTIME_LEGACY_TOKEN_URL: ' False ' }).realtimeLegacyTokenUrl
		).toBe(false);
	});

	it('defaults the VAPID subject to the normalised origin', () => {
		const c = parseConfig({
			...valid,
			ENZARB_PUBLIC_URL: 'https://enzarb.example.com/a/b',
			ENZARB_VAPID_PUBLIC_KEY: 'pub',
			ENZARB_VAPID_PRIVATE_KEY: 'priv'
		});
		expect(c.vapid?.subject).toBe('https://enzarb.example.com');
	});

	it('derives optional keys as optional in the Config type', () => {
		expectTypeOf<Config['githubAppId']>().toEqualTypeOf<string | undefined>();
		expectTypeOf<Config['vapid']>().toEqualTypeOf<VapidConfig | undefined>();
		expect(parseConfig(valid).vapid).toBeUndefined();
	});

	it('treats blank values as missing', () => {
		expect(() => parseConfig({ ...valid, DATABASE_URL: '  ' })).toThrow(/DATABASE_URL/);
	});

	it('rejects a malformed or non-http public URL', () => {
		expect(() => parseConfig({ ...valid, ENZARB_PUBLIC_URL: 'nope' })).toThrow(/absolute URL/);
		expect(() => parseConfig({ ...valid, ENZARB_PUBLIC_URL: 'ftp://x' })).toThrow(/http\(s\)/);
	});
});
