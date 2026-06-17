import type { PageServerLoad } from './$types';
import { getAgentToken } from '$remote/projects';

export const load: PageServerLoad = async ({ params }) => {
	let agentToken: string | null = null;
	try {
		agentToken = await getAgentToken(params.org, params.project);
	} catch {}
	return {
		agentBase: `https://enzarb.dev/agent/${params.project}`,
		agentToken
	};
};
