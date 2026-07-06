import { describe, expect, it } from 'vitest';
import { checkDomainTxt, CHALLENGE_LABEL, CHALLENGE_PREFIX } from './domainVerify';

const FQDN = 'app.example.com';
const TOKEN = 'abc123';

describe('checkDomainTxt', () => {
	it('reports verified when the TXT record matches', async () => {
		const result = await checkDomainTxt(FQDN, TOKEN, async (name) => {
			expect(name).toBe(`${CHALLENGE_LABEL}.${FQDN}`);
			return [[`${CHALLENGE_PREFIX}${TOKEN}`]];
		});
		expect(result).toEqual({ status: 'verified' });
	});

	it('joins chunked TXT strings before comparing', async () => {
		const value = `${CHALLENGE_PREFIX}${TOKEN}`;
		const result = await checkDomainTxt(FQDN, TOKEN, async () => [
			[value.slice(0, 5), value.slice(5)]
		]);
		expect(result).toEqual({ status: 'verified' });
	});

	it('reports pending when no record matches the token', async () => {
		const result = await checkDomainTxt(FQDN, TOKEN, async () => [
			[`${CHALLENGE_PREFIX}wrong-token`]
		]);
		expect(result).toEqual({ status: 'pending' });
	});

	it('reports pending (not error) when the record does not exist yet', async () => {
		const result = await checkDomainTxt(FQDN, TOKEN, async () => {
			const err: any = new Error('queryTxt ENOTFOUND');
			err.code = 'ENOTFOUND';
			throw err;
		});
		expect(result).toEqual({ status: 'pending' });
	});

	it('reports pending on ENODATA', async () => {
		const result = await checkDomainTxt(FQDN, TOKEN, async () => {
			const err: any = new Error('queryTxt ENODATA');
			err.code = 'ENODATA';
			throw err;
		});
		expect(result).toEqual({ status: 'pending' });
	});

	it('surfaces other DNS errors instead of masking them as pending', async () => {
		const result = await checkDomainTxt(FQDN, TOKEN, async () => {
			const err: any = new Error('queryTxt SERVFAIL');
			err.code = 'SERVFAIL';
			throw err;
		});
		expect(result).toEqual({ status: 'error', message: 'queryTxt SERVFAIL' });
	});
});
