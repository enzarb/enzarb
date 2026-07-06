import { resolveTxt as dnsResolveTxt } from 'node:dns/promises';

// Mirrors operator/internal/controller/environment_controller.go's
// challengeLabel/challengePrefix and verifyDomainTXT exactly, so the app can
// cheaply pre-check DNS itself before asking the operator (via the
// enzarb.io/recheck-domains annotation) to do the authoritative check and
// claim the domain.
export const CHALLENGE_LABEL = '_enzarb-challenge';
export const CHALLENGE_PREFIX = 'enzarb-verify=';

export type TxtResolver = (name: string) => Promise<string[][]>;

export type DomainCheckResult =
	| { status: 'verified' }
	| { status: 'pending' }
	| { status: 'error'; message: string };

// Node splits long TXT strings into multiple chunks per record; join each
// record's chunks before comparing, same as a single DNS TXT string value.
export async function checkDomainTxt(
	fqdn: string,
	token: string,
	resolveTxt: TxtResolver = dnsResolveTxt
): Promise<DomainCheckResult> {
	const name = `${CHALLENGE_LABEL}.${fqdn}`;
	const want = `${CHALLENGE_PREFIX}${token}`;
	let records: string[][];
	try {
		records = await resolveTxt(name);
	} catch (err: any) {
		if (err?.code === 'ENOTFOUND' || err?.code === 'ENODATA') {
			return { status: 'pending' };
		}
		return { status: 'error', message: err instanceof Error ? err.message : 'DNS lookup failed' };
	}
	const matched = records.some((chunks) => chunks.join('') === want);
	return matched ? { status: 'verified' } : { status: 'pending' };
}
