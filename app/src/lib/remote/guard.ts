import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import type { Privilege } from '$lib/privileges';

// resolveOrg returns the caller's membership of the org named by the [namespace]
// route param (or an explicitly passed slug, for commands that receive it as an
// argument), asserting an authenticated session and membership. Use for any
// action a plain member may perform (reads, workspace use).
export function resolveOrg(namespace?: string) {
	const { locals, params } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const slug = namespace ?? params.namespace;
	const org = locals.session.orgs.find((o) => o.slug === slug);
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
