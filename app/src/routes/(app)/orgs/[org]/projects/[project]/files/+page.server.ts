import type { PageServerLoad } from './$types';
import { getGitTree } from '$remote/files';
import { getAgentToken } from '$remote/projects';

export const load: PageServerLoad = async ({ params }) => {
	const [gitTree, agentToken] = await Promise.allSettled([
		getGitTree(params.org, params.project, '', 'main'),
		getAgentToken(params.org, params.project)
	]);
	return {
		gitTree: gitTree.status === 'fulfilled' ? gitTree.value : [],
		agentToken: agentToken.status === 'fulfilled' ? agentToken.value : null,
		agentBase: `https://enzarb.dev/agent/${params.project}`
	};
};
