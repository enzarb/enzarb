import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import type { Privilege } from '$lib/privileges';

// resolveOrg returns the caller's membership of the org named by the [namespace]
// route param, asserting an authenticated session and membership. Use for any
// action a plain member may perform (reads, workspace use).
export function resolveOrg() {
	const { locals, params } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const org = locals.session.orgs.find((o) => o.slug === params.namespace);
	if (!org) error(403, 'Not a member of this organization');
	return org;
}

// requirePrivilege resolves the org and asserts the caller's role grants the
// given privilege, erroring 403 otherwise. Returns the membership on success.
export function requirePrivilege(privilege: Privilege) {
	const org = resolveOrg();
	if (!org.privileges.includes(privilege)) {
		error(403, `Requires "${privilege}" privilege`);
	}
	return org;
}
