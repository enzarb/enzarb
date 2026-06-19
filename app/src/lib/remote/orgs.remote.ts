import { form, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error, redirect } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { createOrgWithAdmin, isUsernameAvailable } from '$lib/orgs';

const CreateOrgSchema = z.object({
	slug: z.string().min(2).max(63).regex(/^[a-z0-9][a-z0-9-]*[a-z0-9]$/),
	displayName: z.string().min(1).max(100)
});

export const createOrg = form(CreateOrgSchema, async ({ slug, displayName }) => {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const available = await isUsernameAvailable(slug);
	if (!available) error(422, 'That organization slug is already taken.');
	await createOrgWithAdmin(slug, displayName, locals.session.userId);
	redirect(303, '/dashboard');
});

export const createOrgCommand = command(CreateOrgSchema, async ({ slug, displayName }) => {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const available = await isUsernameAvailable(slug);
	if (!available) error(422, 'That organization slug is already taken.');
	return createOrgWithAdmin(slug, displayName, locals.session.userId);
});
