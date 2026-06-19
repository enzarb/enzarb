import { sql } from './db';
import { createOrganization, waitForOrganizationReady } from './k8s';

// Create an org and add a user as admin. No auth check — call only from trusted server-side code.
// Also creates the cluster-scoped Organization CR and blocks (bounded) until the
// operator has provisioned the org namespace, so project creation isn't racy.
export async function createOrgWithAdmin(
	slug: string,
	displayName: string,
	userId: string,
	tier: 'free' | 'pro' = 'free'
): Promise<string> {
	const rows = await sql`
		INSERT INTO organizations (slug, display_name, tier)
		VALUES (${slug}, ${displayName}, ${tier})
		RETURNING id
	`;
	const orgId: string = rows[0].id;
	await sql`
		INSERT INTO org_members (org_id, user_id, role)
		VALUES (${orgId}, ${userId}, 'admin')
		ON CONFLICT (org_id, user_id) DO NOTHING
	`;

	await createOrganization(orgId, slug, displayName);
	// Block up to ~30s for the namespace; on timeout we proceed anyway — the org
	// row exists and the CR will reconcile shortly. Project creation has its own
	// readiness guard for the lingering race.
	const ready = await waitForOrganizationReady(orgId);
	if (!ready) {
		console.warn(`org ${slug} (${orgId}) namespace not Ready within timeout; provisioning continues`);
	}
	return orgId;
}

// Check whether a username/org slug is available.
export async function isUsernameAvailable(username: string): Promise<boolean> {
	const users = await sql`SELECT 1 FROM users WHERE username = ${username} LIMIT 1`;
	const orgs = await sql`SELECT 1 FROM organizations WHERE slug = ${username} LIMIT 1`;
	return users.length === 0 && orgs.length === 0;
}
