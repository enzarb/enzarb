import { describe, expect, it, vi } from 'vitest';
import {
	commitProposal,
	createProposalBranch,
	githubRepoApi,
	listRepoWorkflows,
	openProposal,
	proposalBranch,
	RepoError,
	type ChangedFile,
	type RepoApi
} from './workflow-repo.ts';

/** In-memory GitHub repository: branches map to file maps. */
function fakeRepo(opts: { role?: string; files?: Record<string, string> } = {}) {
	const branches = new Map<string, Map<string, string>>([
		['main', new Map(Object.entries(opts.files ?? {}))]
	]);
	const commits: { branch: string; message: string; paths: string[] }[] = [];
	const prs: { head: string; base: string; title: string; body: string }[] = [];
	const api: RepoApi = {
		defaultBranch: async () => 'main',
		listDir: async (dir, ref) =>
			[...(branches.get(ref)?.keys() ?? [])]
				.filter((p) => p.startsWith(`${dir}/`) && !p.slice(dir.length + 1).includes('/'))
				.map((p) => ({ name: p.slice(dir.length + 1), path: p, type: 'file' })),
		getFile: async (path, ref) => {
			const text = branches.get(ref)?.get(path);
			return text === undefined ? null : { path, text, sha: 'x' };
		},
		branchSha: async (b) => (branches.has(b) ? `sha-${b}` : null),
		createBranch: async (b) => {
			if (branches.has(b)) throw new RepoError('exists', 422);
			branches.set(b, new Map(branches.get('main')));
		},
		commitFiles: async (b, files: ChangedFile[], message) => {
			const m = branches.get(b);
			if (!m) throw new RepoError('no branch');
			for (const f of files) m.set(f.path, f.text);
			commits.push({ branch: b, message, paths: files.map((f) => f.path) });
			return 'c1';
		},
		openPullRequest: async (pr) => {
			prs.push(pr);
			return { number: 7, url: 'https://github.com/o/r/pull/7' };
		},
		permission: async () => opts.role ?? 'write'
	};
	return { api, branches, commits, prs };
}

const DEF = { path: '.enzarb/workflows/issue.yaml', text: 'version: 1\n' };

describe('listRepoWorkflows', () => {
	it('lists definitions and prompts of the default branch, ignoring other files', async () => {
		const { api } = fakeRepo({
			files: {
				'.enzarb/workflows/issue.yaml': 'a',
				'.enzarb/workflows/notes.txt': 'b',
				'.enzarb/prompts/fix.md': 'c',
				'README.md': 'd'
			}
		});
		expect(await listRepoWorkflows(api)).toEqual({
			defaultBranch: 'main',
			workflows: ['.enzarb/workflows/issue.yaml'],
			prompts: ['.enzarb/prompts/fix.md']
		});
	});
});

describe('proposing a change', () => {
	it('branches, commits and opens a PR without touching the default branch', async () => {
		const r = fakeRepo({ files: { [DEF.path]: 'old' } });
		const { branch, base } = await createProposalBranch(r.api, 'octo', 'Issue Flow');
		expect(base).toBe('main');
		expect(branch).toMatch(/^enzarb\/workflow-issue-flow-[a-f0-9]{6}$/);
		const prompt = { path: '.enzarb/prompts/fix.md', text: 'Fix it.' };
		await commitProposal(r.api, 'octo', branch, 'issue', [DEF, prompt]);
		const pr = await openProposal(r.api, 'octo', branch, 'issue', [DEF.path, prompt.path]);
		expect(pr.url).toBe('https://github.com/o/r/pull/7');
		expect(r.commits).toEqual([
			{
				branch,
				message: 'chore(workflows): update issue workflow',
				paths: [DEF.path, prompt.path]
			}
		]);
		expect(r.prs[0]).toMatchObject({
			head: branch,
			base: 'main',
			title: 'chore(workflows): update issue workflow'
		});
		expect(r.branches.get('main')!.get(DEF.path)).toBe('old');
		expect(r.branches.get(branch)!.get(DEF.path)).toBe('version: 1\n');
	});

	it.each(['read', 'triage', 'none'])('refuses a user with %s access', async (role) => {
		const r = fakeRepo({ role });
		const err = await createProposalBranch(r.api, 'octo', 'issue').catch((e) => e);
		expect(err).toBeInstanceOf(RepoError);
		expect(err.status).toBe(403);
		expect(r.branches.size).toBe(1);
		await expect(
			commitProposal(r.api, 'octo', 'enzarb/workflow-issue-abcdef', 'issue', [DEF])
		).rejects.toThrow(/write access/);
	});

	it.each(['admin', 'maintain'])('accepts %s', async (role) => {
		const r = fakeRepo({ role });
		await expect(createProposalBranch(r.api, 'octo', 'issue')).resolves.toBeTruthy();
	});

	it('never commits to the default branch or any non-proposal branch', async () => {
		const r = fakeRepo();
		for (const b of ['main', 'enzarb/issue-1', 'enzarb/workflow-../main-abcdef'])
			await expect(commitProposal(r.api, 'octo', b, 'issue', [DEF])).rejects.toThrow(
				/proposal branch/
			);
		expect(r.commits).toEqual([]);
	});

	it('commits only workflow and prompt files', async () => {
		const r = fakeRepo();
		const { branch } = await createProposalBranch(r.api, 'octo', 'issue');
		for (const path of [
			'.github/workflows/ci.yaml',
			'.enzarb/prompts/../../x.md',
			'.enzarb/workflows/a/b.yaml',
			'README.md'
		])
			await expect(
				commitProposal(r.api, 'octo', branch, 'issue', [{ path, text: 'x' }])
			).rejects.toThrow(/not a workflow definition or prompt file/);
		await expect(commitProposal(r.api, 'octo', branch, 'issue', [])).rejects.toThrow(/Nothing/);
		expect(r.commits).toEqual([]);
	});

	it('slugs branch names', () => {
		expect(proposalBranch('Issue: Fast/Path!', 'abc123')).toBe(
			'enzarb/workflow-issue-fast-path-abc123'
		);
		expect(proposalBranch('***', 'abc123')).toBe('enzarb/workflow-workflow-abc123');
	});
});

describe('githubRepoApi', () => {
	const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

	it('commits several files in one commit through the git data API', async () => {
		const calls: [string, string, unknown][] = [];
		const f = vi.fn(async (url: string, init: RequestInit) => {
			const path = url.replace('https://api.github.com/repos/o/r', '');
			calls.push([init.method!, path, init.body ? JSON.parse(init.body as string) : undefined]);
			if (path.startsWith('/git/ref/heads/')) return json({ object: { sha: 'head' } });
			if (path === '/git/commits/head') return json({ tree: { sha: 'tree0' } });
			if (path === '/git/trees') return json({ sha: 'tree1' }, 201);
			if (path === '/git/commits') return json({ sha: 'new' }, 201);
			if (path.startsWith('/git/refs/heads/')) return json({});
			return json({}, 404);
		});
		const api = githubRepoApi('tok', 'o/r', f as unknown as typeof fetch);
		const sha = await api.commitFiles('enzarb/workflow-x-abcdef', [DEF], 'msg');
		expect(sha).toBe('new');
		expect(calls.map(([m, p]) => `${m} ${p}`)).toEqual([
			'GET /git/ref/heads/enzarb/workflow-x-abcdef',
			'GET /git/commits/head',
			'POST /git/trees',
			'POST /git/commits',
			'PATCH /git/refs/heads/enzarb/workflow-x-abcdef'
		]);
		expect(calls[2][2]).toEqual({
			base_tree: 'tree0',
			tree: [{ path: DEF.path, mode: '100644', type: 'blob', content: DEF.text }]
		});
		expect(calls[3][2]).toEqual({ message: 'msg', tree: 'tree1', parents: ['head'] });
		expect(calls[4][2]).toEqual({ sha: 'new', force: false });
		const headers = (f.mock.calls[0][1] as RequestInit).headers as Record<string, string>;
		expect(headers.Authorization).toBe('Bearer tok');
	});

	it('decodes file contents and maps 404 to null', async () => {
		const f = vi.fn(async (url: string) =>
			url.includes('missing')
				? json({}, 404)
				: json({ type: 'file', sha: 's', content: Buffer.from('héllo').toString('base64') })
		);
		const api = githubRepoApi('tok', 'o/r', f as unknown as typeof fetch);
		expect(await api.getFile('.enzarb/prompts/a.md', 'main')).toEqual({
			path: '.enzarb/prompts/a.md',
			sha: 's',
			text: 'héllo'
		});
		expect(await api.getFile('.enzarb/prompts/missing.md', 'main')).toBeNull();
		expect(f.mock.calls[0][0]).toBe(
			'https://api.github.com/repos/o/r/contents/.enzarb/prompts/a.md?ref=main'
		);
	});

	it('reads the role, falling back to the legacy permission', async () => {
		const role = (body: unknown, status = 200) =>
			githubRepoApi('t', 'o/r', (async () =>
				json(body, status)) as unknown as typeof fetch).permission('octo');
		expect(await role({ permission: 'write', role_name: 'maintain' })).toBe('maintain');
		expect(await role({ permission: 'admin' })).toBe('admin');
		expect(await role({}, 404)).toBe('none');
	});
});
