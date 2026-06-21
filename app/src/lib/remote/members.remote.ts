import { query, command } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { sql } from '$lib/db';
import { resolveOrg, requirePrivilege } from './guard';
import { PRIVILEGES, BUILTIN_ROLE_NAMES, isPrivilege } from '$lib/privileges';

// List the org's members with their email and assigned role. Visible to any member.
export const getMembers = query(async () => {
	const org = resolveOrg();
	return sql<{ userId: string; email: string; username: string | null; role: string }[]>`
		SELECT u.id AS "userId", u.email, u.username, om.role
		FROM org_members om
		JOIN users u ON u.id = om.user_id
		WHERE om.org_id = ${org.id}
		ORDER BY u.email
	`;
});

// List the org's roles and their privilege sets. Visible to any member.
export const getRoles = query(async () => {
	const org = resolveOrg();
	return sql<{ name: string; privileges: string[]; builtin: boolean }[]>`
		SELECT name, privileges, builtin
		FROM org_roles
		WHERE org_id = ${org.id}
		ORDER BY builtin DESC, name
	`;
});

// countMemberManageHolders returns how many members hold a role granting
// member.manage — used to prevent an org from locking itself out of admin.
async function countMemberManageHolders(orgId: string): Promise<number> {
	const rows = await sql<{ n: number }[]>`
		SELECT COUNT(*)::int AS n
		FROM org_members om
		JOIN org_roles r ON r.org_id = om.org_id AND r.name = om.role
		WHERE om.org_id = ${orgId} AND 'member.manage' = ANY(r.privileges)
	`;
	return rows[0]?.n ?? 0;
}

// Assign a role to an existing member. Requires member.manage. Guards against
// removing the last member who can manage members.
export const setMemberRole = command(
	z.object({ userId: z.string().uuid(), role: z.string() }),
	async ({ userId, role }) => {
		const org = requirePrivilege('member.manage');

		const roleRows = await sql<{ privileges: string[] }[]>`
			SELECT privileges FROM org_roles WHERE org_id = ${org.id} AND name = ${role}
		`;
		if (!roleRows.length) error(404, `Role "${role}" does not exist`);

		const memberRows = await sql<{ role: string }[]>`
			SELECT role FROM org_members WHERE org_id = ${org.id} AND user_id = ${userId}
		`;
		if (!memberRows.length) error(404, 'Member not found');

		// If we'd be removing member.manage from the last holder, block it.
		const losingManage =
			!roleRows[0].privileges.includes('member.manage') &&
			(await holderHasManage(org.id, memberRows[0].role));
		if (losingManage && (await countMemberManageHolders(org.id)) <= 1) {
			error(409, 'Cannot change role: the org must keep at least one member who can manage members');
		}

		await sql`
			UPDATE org_members SET role = ${role}
			WHERE org_id = ${org.id} AND user_id = ${userId}
		`;
		return { userId, role };
	}
);

async function holderHasManage(orgId: string, roleName: string): Promise<boolean> {
	const rows = await sql<{ has: boolean }[]>`
		SELECT 'member.manage' = ANY(privileges) AS has
		FROM org_roles WHERE org_id = ${orgId} AND name = ${roleName}
	`;
	return rows[0]?.has ?? false;
}

const RoleNameSchema = z
	.string()
	.min(1)
	.max(32)
	.regex(/^[a-z][a-z0-9-]*$/, 'lowercase letters, digits and dashes only');

// Set a role's privilege set. Requires role.manage. Works on builtin and custom
// roles alike. Guards against stripping member.manage from the org entirely.
export const updateRolePrivileges = command(
	z.object({ name: z.string(), privileges: z.array(z.string()) }),
	async ({ name, privileges }) => {
		const org = requirePrivilege('role.manage');

		const invalid = privileges.filter((p) => !isPrivilege(p));
		if (invalid.length) error(400, `Unknown privileges: ${invalid.join(', ')}`);

		const existing = await sql<{ privileges: string[] }[]>`
			SELECT privileges FROM org_roles WHERE org_id = ${org.id} AND name = ${name}
		`;
		if (!existing.length) error(404, `Role "${name}" does not exist`);

		// If this role currently provides the org's only member.manage coverage and
		// we're removing it, block to avoid an unrecoverable lockout.
		if (existing[0].privileges.includes('member.manage') && !privileges.includes('member.manage')) {
			if ((await countMemberManageHolders(org.id)) <= (await membersInRole(org.id, name))) {
				error(409, 'Cannot remove member.manage: no other role would retain it');
			}
		}

		await sql`
			UPDATE org_roles SET privileges = ${privileges}
			WHERE org_id = ${org.id} AND name = ${name}
		`;
		return { name, privileges };
	}
);

async function membersInRole(orgId: string, roleName: string): Promise<number> {
	const rows = await sql<{ n: number }[]>`
		SELECT COUNT(*)::int AS n FROM org_members
		WHERE org_id = ${orgId} AND role = ${roleName}
	`;
	return rows[0]?.n ?? 0;
}

// Create a custom role. Requires role.manage.
export const createRole = command(
	z.object({ name: RoleNameSchema, privileges: z.array(z.string()).default([]) }),
	async ({ name, privileges }) => {
		const org = requirePrivilege('role.manage');
		const invalid = privileges.filter((p) => !isPrivilege(p));
		if (invalid.length) error(400, `Unknown privileges: ${invalid.join(', ')}`);

		const dup = await sql`SELECT 1 FROM org_roles WHERE org_id = ${org.id} AND name = ${name}`;
		if (dup.length) error(409, `Role "${name}" already exists`);

		await sql`
			INSERT INTO org_roles (org_id, name, privileges, builtin)
			VALUES (${org.id}, ${name}, ${privileges}, false)
		`;
		return { name, privileges };
	}
);

// Delete a custom role. Requires role.manage. Builtin roles and roles still in
// use by a member cannot be deleted.
export const deleteRole = command(z.object({ name: z.string() }), async ({ name }) => {
	const org = requirePrivilege('role.manage');
	if (BUILTIN_ROLE_NAMES.includes(name)) error(409, 'Builtin roles cannot be deleted');

	if ((await membersInRole(org.id, name)) > 0) {
		error(409, 'Cannot delete a role that is still assigned to members');
	}
	await sql`DELETE FROM org_roles WHERE org_id = ${org.id} AND name = ${name}`;
	return { name };
});

// Exposed so the UI can render the full privilege catalog with checkboxes.
export const getPrivilegeCatalog = query(async () => {
	resolveOrg();
	return PRIVILEGES as readonly string[];
});
