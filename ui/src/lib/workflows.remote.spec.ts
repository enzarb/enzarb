import { isHttpError } from '@sveltejs/kit';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const h = vi.hoisted(() => ({
	project: {
		repositories: [
			{ connector: 'github', url: 'https://github.com/Acme/Widgets' },
			{ connector: 'gitlab', url: 'https://gitlab.com/acme/other' }
		]
	},
	token: 'tok' as string | null,
	tokenCalls: [] as [string, unknown][],
	api: {} as Record<string, ReturnType<typeof vi.fn>>
}));

vi.mock('$app/server', () => ({
	command: (_schema: unknown, handler: object) =>
		Object.assign(handler, { __: { type: 'command' } }),
	query: (_schema: unknown, handler: object) => Object.assign(handler, { __: { type: 'query' } }),
	getRequestEvent: () => ({ locals: {} })
}));
vi.mock('#lib/server/owned-project.ts', () => ({
	NameArg: {},
	ownedProject: async () => ({ user: { login: 'octo' }, project: h.project })
}));
vi.mock('#lib/server/config.ts', () => ({
	getConfig: () => ({ githubAppId: '1', githubAppPrivateKey: 'pem' })
}));
vi.mock('#lib/server/github-app.ts', () => ({
	repoInstallationToken: async (repo: string, perms: unknown) => {
		h.tokenCalls.push([repo, perms]);
		return h.token;
	}
}));
vi.mock('#lib/server/workflow-repo.ts', async (orig) => ({
	...(await orig<object>()),
	githubRepoApi: () => h.api
}));

import {
	commitWorkflowProposal,
	projectWorkflows,
	startWorkflowProposal,
	workflowFile
} from './workflows.remote.ts';

type Fn = (arg: unknown) => Promise<unknown>;
const call = (f: unknown, arg: unknown) => (f as Fn)(arg);

beforeEach(() => {
	h.token = 'tok';
	h.tokenCalls = [];
	h.api = {
		defaultBranch: vi.fn(async () => 'main'),
		listDir: vi.fn(async (dir: string) =>
			dir === '.enzarb/workflows' ? [{ name: 'issue.yaml', path: '', type: 'file' }] : []
		),
		getFile: vi.fn(async (path: string) => ({ path, text: 'x', sha: 's' })),
		permission: vi.fn(async () => 'read'),
		branchSha: vi.fn(async () => 'sha'),
		createBranch: vi.fn(),
		commitFiles: vi.fn()
	};
});

describe('workflows remote functions', () => {
	it('lists the GitHub repositories of the project with a read-only token', async () => {
		expect(await call(projectWorkflows, 'p')).toEqual([
			{
				repo: 'acme/widgets',
				defaultBranch: 'main',
				workflows: ['.enzarb/workflows/issue.yaml'],
				prompts: []
			}
		]);
		expect(h.tokenCalls).toEqual([['acme/widgets', { contents: 'read', metadata: 'read' }]]);
	});

	it('reports a repository the App cannot read instead of failing the list', async () => {
		h.token = null;
		const [r] = (await call(projectWorkflows, 'p')) as { error?: string }[];
		expect(r.error).toMatch(/GitHub App/);
	});

	it('404s for a repository outside the project', async () => {
		const err = await call(workflowFile, {
			name: 'p',
			repo: 'evil/repo',
			path: '.enzarb/workflows/issue.yaml'
		}).catch((e) => e);
		expect(isHttpError(err, 404)).toBe(true);
		expect(h.tokenCalls).toEqual([]);
	});

	it('reads only workflow and prompt files', async () => {
		const err = await call(workflowFile, {
			name: 'p',
			repo: 'acme/widgets',
			path: '.github/workflows/ci.yaml'
		}).catch((e) => e);
		expect(isHttpError(err, 400)).toBe(true);
		expect(
			await call(workflowFile, { name: 'p', repo: 'Acme/Widgets', path: '.enzarb/prompts/a.md' })
		).toEqual({ path: '.enzarb/prompts/a.md', text: 'x' });
	});

	it('403s a proposal from a user without write access', async () => {
		const err = await call(startWorkflowProposal, {
			name: 'p',
			repo: 'acme/widgets',
			workflow: 'issue'
		}).catch((e) => e);
		expect(isHttpError(err, 403)).toBe(true);
		expect(h.api.createBranch).not.toHaveBeenCalled();
		expect(h.tokenCalls[0][1]).toEqual({
			contents: 'write',
			pull_requests: 'write',
			metadata: 'read'
		});
	});

	it('refuses to commit to the default branch', async () => {
		h.api.permission.mockResolvedValue('write');
		const err = await call(commitWorkflowProposal, {
			name: 'p',
			repo: 'acme/widgets',
			workflow: 'issue',
			branch: 'main',
			files: [{ path: '.enzarb/workflows/issue.yaml', text: 'x' }]
		}).catch((e) => e);
		expect(isHttpError(err, 502)).toBe(true);
		expect(h.api.commitFiles).not.toHaveBeenCalled();
	});
});
