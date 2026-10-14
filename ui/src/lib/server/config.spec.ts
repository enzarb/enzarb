import { describe, expect, it } from 'vitest';

import { parseConfig } from './config.ts';

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
			componentVersions: { components: {} }
		});
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

	it('lists every missing variable at once', () => {
		expect(() => parseConfig({ DATABASE_URL: valid.DATABASE_URL })).toThrow(
			/ENZARB_SESSION_JWT_SECRET, ENZARB_REALTIME_JWT_SECRET, GITHUB_OAUTH_CLIENT_ID, GITHUB_OAUTH_CLIENT_SECRET, ENZARB_PUBLIC_URL/
		);
	});

	it('treats blank values as missing', () => {
		expect(() => parseConfig({ ...valid, DATABASE_URL: '  ' })).toThrow(/DATABASE_URL/);
	});

	it('rejects a malformed or non-http public URL', () => {
		expect(() => parseConfig({ ...valid, ENZARB_PUBLIC_URL: 'nope' })).toThrow(/absolute URL/);
		expect(() => parseConfig({ ...valid, ENZARB_PUBLIC_URL: 'ftp://x' })).toThrow(/http\(s\)/);
	});
});
