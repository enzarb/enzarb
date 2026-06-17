import { json } from '@sveltejs/kit';
import { getJwks } from '$lib/jwt';

export const GET = () => {
	return json(getJwks(), {
		headers: { 'Cache-Control': 'public, max-age=300' }
	});
};
