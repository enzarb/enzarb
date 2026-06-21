// Privilege-based authorization. Privileges are a fixed catalog (each maps to a
// server-side check); roles are per-org, editable bags of privileges stored in
// the org_roles table. org_members.role references a role by name.

export const PRIVILEGES = [
	'project.create',
	'project.delete',
	'environment.manage',
	'registry.delete',
	'billing.manage',
	'member.manage',
	'role.manage',
	'org.delete'
] as const;

export type Privilege = (typeof PRIVILEGES)[number];

// Human-readable labels for the role-management UI.
export const PRIVILEGE_LABELS: Record<Privilege, string> = {
	'project.create': 'Create projects',
	'project.delete': 'Delete projects',
	'environment.manage': 'Manage deploy environments & domains',
	'registry.delete': 'Delete registry images',
	'billing.manage': 'Manage billing',
	'member.manage': 'Manage members & assign roles',
	'role.manage': 'Edit roles & privileges',
	'org.delete': 'Delete the organization'
};

// Builtin roles seeded into every org. Their privilege sets remain editable by
// anyone with role.manage; only their existence is protected (can't be deleted).
export const DEFAULT_ROLES: { name: string; privileges: Privilege[] }[] = [
	{
		name: 'member',
		privileges: ['project.create', 'project.delete']
	},
	{
		name: 'manager',
		privileges: [
			'project.create',
			'project.delete',
			'environment.manage',
			'registry.delete',
			'billing.manage'
		]
	},
	{
		name: 'owner',
		privileges: [...PRIVILEGES]
	}
];

export const BUILTIN_ROLE_NAMES = DEFAULT_ROLES.map((r) => r.name);

// The role a brand-new org's creator receives.
export const OWNER_ROLE = 'owner';

export function isPrivilege(value: string): value is Privilege {
	return (PRIVILEGES as readonly string[]).includes(value);
}
