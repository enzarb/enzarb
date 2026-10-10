import { describe, expect, it } from 'vitest';

import { parseConfig } from './config.ts';

const valid = {
	DATABASE_URL: 'postgres://u:p@h:5432/enzarb',
	ENZARB_SESSION_JWT_SECRET: 's3cret',
	GITHUB_OAUTH_CLIENT_ID: 'id',
	GITHUB_OAUTH_CLIENT_SECRET: 'secret',
	ENZARB_PUBLIC_URL: 'https://enzarb.example.com/'
};

describe('parseConfig', () => {
	it('parses a complete environment and normalizes the public URL', () => {
		expect(parseConfig(valid)).toEqual({
			databaseUrl: valid.DATABASE_URL,
			sessionJwtSecret: 's3cret',
			githubOauthClientId: 'id',
			githubOauthClientSecret: 'secret',
			publicUrl: 'https://enzarb.example.com',
			githubAppSlug: 'enzarb'
		});
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
			/ENZARB_SESSION_JWT_SECRET, GITHUB_OAUTH_CLIENT_ID, GITHUB_OAUTH_CLIENT_SECRET, ENZARB_PUBLIC_URL/
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
