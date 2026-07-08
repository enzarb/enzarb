import { query, form, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import {
	listEnvironments,
	createEnvironment,
	deleteEnvironment,
	addCustomDomain,
	removeCustomDomain,
	moveCustomDomain,
	setDefaultEnvironment,
	getEnvironment,
	requestDomainRecheck,
	getGatewayPublicIPs,
	getProject as k8sGetProject
} from '$lib/k8s';
import { sql } from '$lib/db';
import { tiers, config } from '$lib/config';
import { resolveOrg, requirePrivilege } from './guard';
import { checkDomainTxt } from '$lib/domainVerify';

export const getEnvironments = query(z.string().optional(), async () => {
	const { params } = getRequestEvent();
	const org = resolveOrg();
	const [envs, project, gatewayPublicIPs] = await Promise.all([
		listEnvironments(org.id, params.project!),
		k8sGetProject(org.id, params.project!),
		getGatewayPublicIPs()
	]);
	const deployZone = `env.${config.domain}`;
	const defaultEnvSlug = (project as any)?.metadata?.annotations?.['enzarb.io/default-environment'] ?? null;
	return { envs, deployZone, defaultEnvSlug, gatewayPublicIPs };
});

export const createEnv = form(
	z.object({ slug: z.string().min(1).max(63).regex(/^[a-z0-9-]+$/) }),
	async ({ slug }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		const rows = await sql`SELECT tier FROM organizations WHERE id = ${org.id}`;
		const tier = (rows[0]?.tier ?? 'free') as keyof typeof tiers;
		const existing = await listEnvironments(org.id, params.project!);
		if (existing.length >= tiers[tier].maxEnvironments) {
			error(422, `Tier limited to ${tiers[tier].maxEnvironments} environment(s)`);
		}
		if (existing.some((e: any) => e.spec?.slug === slug)) {
			error(409, `Environment "${slug}" already exists`);
		}
		const result = await createEnvironment(org.id, params.project!, slug);
		await getEnvironments(params.project!).refresh();
		return result;
	}
);

export const removeEnv = command(
	z.object({ envName: z.string(), envSlug: z.string() }),
	async ({ envName, envSlug }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		const project = (await k8sGetProject(org.id, params.project!)) as any;
		if (project?.metadata?.annotations?.['enzarb.io/default-environment'] === envSlug) {
			await setDefaultEnvironment(org.id, params.project!, null);
		}
		await deleteEnvironment(org.id, envName);
		await getEnvironments(params.project!).refresh();
	}
);

export const setDefaultEnv = command(
	z.object({ envSlug: z.string().nullable() }),
	async ({ envSlug }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		await setDefaultEnvironment(org.id, params.project!, envSlug);
		await getEnvironments(params.project!).refresh();
	}
);

export const addDomain = form(
	z.object({
		envName: z.string(),
		fqdn: z.string().max(253).regex(/^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,}$/)
	}),
	async ({ envName, fqdn }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		const result = await addCustomDomain(org.id, envName, fqdn);
		await getEnvironments(params.project!).refresh();
		return result;
	}
);

export const removeDomain = command(
	z.object({ envName: z.string(), fqdn: z.string() }),
	async ({ envName, fqdn }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		await removeCustomDomain(org.id, envName, fqdn);
		await getEnvironments(params.project!).refresh();
	}
);

export const moveDomain = command(
	z.object({ fromEnvName: z.string(), toEnvName: z.string(), fqdn: z.string() }),
	async ({ fromEnvName, toEnvName, fqdn }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		await moveCustomDomain(org.id, fromEnvName, toEnvName, fqdn);
		await getEnvironments(params.project!).refresh();
	}
);

function findDomain(env: any, fqdn: string) {
	return (env.status?.domains ?? []).find((d: any) => d.fqdn === fqdn) ?? null;
}

// Requests an immediate recheck (via the enzarb.io/recheck-domains
// annotation) and polls until the operator has processed it, then returns the
// resulting domain status so the UI can show the outcome instead of just
// firing the request blind.
//
// Before ownership is proven, this runs the same TXT check the operator runs
// (checkDomainTxt) here first, so a domain that clearly isn't ready yet gets
// instant "still pending" feedback without waiting on an operator round-trip,
// and only asks the operator to do the authoritative check + claim when the
// local check already sees the record. Once ownership is Verified, there's
// nothing left to precheck client-side (the routing check needs the
// operator's view of the gateway's Service, not this process's), so it just
// asks the operator directly.
export const recheckDomain = command(
	z.object({ envName: z.string(), fqdn: z.string() }),
	async ({ envName, fqdn }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');

		const before = findDomain(await getEnvironment(org.id, envName), fqdn);
		if (!before?.challengeToken) {
			error(409, 'No challenge token yet for this domain — try again shortly');
		}

		if (before.certStatus !== 'Verified') {
			const precheck = await checkDomainTxt(fqdn, before.challengeToken);
			if (precheck.status === 'pending') {
				return { certStatus: 'PendingVerification', lastError: null, routingStatus: null, routingError: null };
			}
			if (precheck.status === 'error') {
				return { certStatus: 'VerificationError', lastError: precheck.message, routingStatus: null, routingError: null };
			}
		}

		// Local check saw the TXT record (or ownership is already Verified);
		// ask the operator to do the authoritative recheck (ownership claim
		// and/or routing) and re-derive certStatus/TLS from it.
		await requestDomainRecheck(org.id, envName);

		const deadline = Date.now() + 20_000;
		let latest = before;
		while (Date.now() < deadline) {
			await new Promise((resolve) => setTimeout(resolve, 1000));
			const env = (await getEnvironment(org.id, envName)) as any;
			latest = findDomain(env, fqdn);
			if (latest?.lastCheckedAt && latest.lastCheckedAt !== before?.lastCheckedAt) break;
		}

		await getEnvironments(params.project!).refresh();

		if (!latest?.lastCheckedAt || latest.lastCheckedAt === before?.lastCheckedAt) {
			error(504, 'Recheck timed out waiting for the operator — try again shortly');
		}
		return {
			certStatus: latest.certStatus as string,
			lastError: (latest.lastError as string) ?? null,
			routingStatus: (latest.routingStatus as string) ?? null,
			routingError: (latest.routingError as string) ?? null
		};
	}
);
