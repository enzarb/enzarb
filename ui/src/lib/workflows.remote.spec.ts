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
	getConfig: () => ({ githubAppId: '1', githubAppPrivateKey: 'pem', sessionJwtSecret: 'sek' })
}));
vi.mock('#lib/server/github-app.ts', () => ({
	createRepoTokenSource: () => ({
		get: async (repo: string, perms: unknown) => {
			h.tokenCalls.push([repo, perms]);
			return h.token;
		}
	})
}));
vi.mock('#lib/server/workflow-repo.ts', async (orig) => ({
	...(await orig<object>()),
	githubRepoApi: () => h.api
}));

import { signProposal } from '#lib/server/workflow-repo.ts';
import {
	commitWorkflowProposal,
	openWorkflowProposal,
	projectWorkflows,
	startWorkflowProposal,
	workflowFile
} from './workflows.remote.ts';

type Fn = (arg: unknown) => Promise<unknown>;
const call = (f: unknown, arg: unknown) => (f as Fn)(arg);
const DEF = { path: '.enzarb/workflows/issue.yaml', text: 'x' };

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
		branchSha: vi.fn(async () => 'base0'),
		createBranch: vi.fn(),
		commitFiles: vi.fn(async () => 'c1'),
		changedPaths: vi.fn(async () => [DEF.path]),
		openPullRequest: vi.fn(async () => ({ number: 7, url: 'u' }))
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
		expect(h.api.permission).toHaveBeenCalledWith('octo');
	});

	it('hides repositories the user has no access to', async () => {
		h.api.permission.mockResolvedValue('none');
		const [r] = (await call(projectWorkflows, 'p')) as { error?: string; workflows?: unknown }[];
		expect(r.error).toMatch(/no access/);
		expect(r.workflows).toBeUndefined();
		const err = await call(workflowFile, {
			name: 'p',
			repo: 'acme/widgets',
			path: DEF.path
		}).catch((e) => e);
		expect(isHttpError(err, 403)).toBe(true);
		expect(h.api.getFile).not.toHaveBeenCalled();
	});

	it('reports a repository the App cannot read instead of failing the list', async () => {
		h.token = null;
		const [r] = (await call(projectWorkflows, 'p')) as { error?: string }[];
		expect(r.error).toMatch(/GitHub App/);
	});

	it('404s for a repository outside the project', async () => {
		const err = await call(workflowFile, { name: 'p', repo: 'evil/repo', path: DEF.path }).catch(
			(e) => e
		);
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

	it('runs the three steps chained by the token', async () => {
		h.api.permission.mockResolvedValue('write');
		const arg = { name: 'p', repo: 'acme/widgets', workflow: 'issue' };
		const s = (await call(startWorkflowProposal, arg)) as { token: string };
		const c = (await call(commitWorkflowProposal, { ...arg, token: s.token, files: [DEF] })) as {
			token: string;
		};
		expect(await call(openWorkflowProposal, { ...arg, token: c.token })).toEqual({
			number: 7,
			url: 'u'
		});
	});

	it('refuses a token minted for another project or a forged branch', async () => {
		h.api.permission.mockResolvedValue('write');
		const claims = {
			project: 'other',
			repo: 'acme/widgets',
			login: 'octo',
			branch: 'enzarb/workflow-issue-abcdef',
			base: 'main',
			baseSha: 'base0',
			exp: Date.now() + 60_000
		};
		for (const token of [
			signProposal('sek', claims),
			signProposal('sek', { ...claims, project: 'p', branch: 'main' }),
			signProposal('wrong', { ...claims, project: 'p' })
		]) {
			const err = await call(commitWorkflowProposal, {
				name: 'p',
				repo: 'acme/widgets',
				workflow: 'issue',
				token,
				files: [DEF]
			}).catch((e) => e);
			expect(isHttpError(err, 400)).toBe(true);
		}
		expect(h.api.commitFiles).not.toHaveBeenCalled();
	});

	it('rejects oversize changes before any GitHub call', async () => {
		const big = 'x'.repeat(250 * 1024);
		const files = Array.from({ length: 5 }, (_, i) => ({
			path: `.enzarb/prompts/p${i}.md`,
			text: big
		}));
		const err = await call(commitWorkflowProposal, {
			name: 'p',
			repo: 'acme/widgets',
			workflow: 'issue',
			token: 'x',
			files
		}).catch((e) => e);
		expect(isHttpError(err, 413)).toBe(true);
		expect(h.tokenCalls).toEqual([]);
	});
});
