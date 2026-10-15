import { describe, expect, it, vi } from 'vitest';
import {
	checkFiles,
	commitProposal,
	createProposalBranch,
	githubRepoApi,
	listRepoWorkflows,
	openProposal,
	proposalBranch,
	RepoError,
	requireRead,
	signProposal,
	verifyProposal,
	type ChangedFile,
	type ProposalScope,
	type RepoApi
} from './workflow-repo.ts';

/** In-memory GitHub repository: branches with a head sha and a file map. */
function fakeRepo(opts: { role?: string; files?: Record<string, string> } = {}) {
	let n = 0;
	const branches = new Map<string, { sha: string; files: Map<string, string> }>([
		['main', { sha: 'base0', files: new Map(Object.entries(opts.files ?? {})) }]
	]);
	const snapshots = new Map<string, Map<string, string>>([['base0', branches.get('main')!.files]]);
	const commits: { branch: string; message: string; paths: string[] }[] = [];
	const prs: { head: string; base: string; title: string; body: string }[] = [];
	const api: RepoApi = {
		defaultBranch: async () => 'main',
		listDir: async (dir, ref) =>
			[...(branches.get(ref)?.files.keys() ?? [])]
				.filter((p) => p.startsWith(`${dir}/`) && !p.slice(dir.length + 1).includes('/'))
				.map((p) => ({ name: p.slice(dir.length + 1), path: p, type: 'file' })),
		getFile: async (path, ref) => {
			const text = branches.get(ref)?.files.get(path);
			return text === undefined ? null : { path, text, sha: 'x' };
		},
		branchSha: async (b) => branches.get(b)?.sha ?? null,
		createBranch: async (b, sha) => {
			if (branches.has(b)) throw new RepoError('exists', 422);
			branches.set(b, { sha, files: new Map(snapshots.get(sha)) });
		},
		commitFiles: async (b, head, files: ChangedFile[], message) => {
			const br = branches.get(b)!;
			if (br.sha !== head) throw new RepoError('moved', 422);
			for (const f of files) br.files.set(f.path, f.text);
			br.sha = `c${++n}`;
			snapshots.set(br.sha, new Map(br.files));
			commits.push({ branch: b, message, paths: files.map((f) => f.path) });
			return br.sha;
		},
		changedPaths: async (base, head) => {
			const a = snapshots.get(base)!;
			const b = snapshots.get(head)!;
			return [...new Set([...a.keys(), ...b.keys()])].filter((k) => a.get(k) !== b.get(k)).sort();
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
const PROMPT = { path: '.enzarb/prompts/fix.md', text: 'Fix it.' };
const scope: ProposalScope = { project: 'p', repo: 'o/r', login: 'octo', secret: 's3cret' };

describe('listRepoWorkflows', () => {
	it('lists only the paths the graph crate accepts', async () => {
		const { api } = fakeRepo({
			files: {
				'.enzarb/workflows/issue.yaml': 'a',
				'.enzarb/workflows/a.b.yaml': 'a',
				'.enzarb/workflows/notes.txt': 'b',
				'.enzarb/prompts/fix.md': 'c',
				'.enzarb/prompts/a.b.md': 'c',
				'README.md': 'd'
			}
		});
		expect(await listRepoWorkflows(api)).toEqual({
			defaultBranch: 'main',
			workflows: ['.enzarb/workflows/issue.yaml'],
			prompts: ['.enzarb/prompts/fix.md']
		});
	});

	it('requires some access to read', async () => {
		await expect(requireRead(fakeRepo({ role: 'none' }).api, 'octo')).rejects.toThrow(/no access/);
		await expect(requireRead(fakeRepo({ role: 'read' }).api, 'octo')).resolves.toBeUndefined();
	});
});

describe('proposal token', () => {
	const claims = {
		project: 'p',
		repo: 'o/r',
		login: 'octo',
		branch: 'enzarb/workflow-issue-abcdef',
		base: 'main',
		baseSha: 'base0',
		exp: 2000
	};
	const t = signProposal('s3cret', claims);

	it('round-trips for the same project, repository and user', () => {
		expect(verifyProposal('s3cret', t, scope, 1000)).toEqual(claims);
	});

	it.each([
		['another user', { ...scope, login: 'mallory' }, 1000, 's3cret', t],
		['another repo', { ...scope, repo: 'o/other' }, 1000, 's3cret', t],
		['another project', { ...scope, project: 'q' }, 1000, 's3cret', t],
		['expired', scope, 3000, 's3cret', t],
		['another secret', scope, 1000, 'other', t],
		[
			'a forged payload',
			scope,
			1000,
			's3cret',
			`${Buffer.from(JSON.stringify({ ...claims, branch: 'main' })).toString('base64url')}.${t.split('.')[1]}`
		],
		['garbage', scope, 1000, 's3cret', 'x.y.z']
	])('rejects %s', (_n, expect_, now, secret, token) => {
		expect(() => verifyProposal(secret, token, expect_, now)).toThrow(/expired or is invalid/);
	});
});

describe('proposing a change', () => {
	async function full(r: ReturnType<typeof fakeRepo>, files: ChangedFile[]) {
		const { branch, token } = await createProposalBranch(r.api, scope, 'Issue Flow');
		const c = await commitProposal(r.api, scope, token, 'Issue Flow', files);
		const pr = await openProposal(r.api, scope, c.token, 'Issue Flow');
		return { branch, pr };
	}

	it('branches, commits and opens a PR without touching the default branch', async () => {
		const r = fakeRepo({ files: { [DEF.path]: 'old' } });
		const { branch, pr } = await full(r, [DEF, PROMPT]);
		expect(branch).toMatch(/^enzarb\/workflow-issue-flow-[a-f0-9]{6}$/);
		expect(pr.url).toBe('https://github.com/o/r/pull/7');
		expect(r.commits).toEqual([
			{
				branch,
				message: 'chore(workflows): update Issue Flow workflow\n\nProposed-by: @octo via enzarb',
				paths: [DEF.path, PROMPT.path]
			}
		]);
		expect(r.prs[0]).toMatchObject({
			head: branch,
			base: 'main',
			title: 'chore(workflows): update Issue Flow workflow'
		});
		// The PR lists what GitHub reports changed, not what the client claimed.
		expect(r.prs[0].body).toContain(`\`${PROMPT.path}\``);
		expect(r.branches.get('main')!.files.get(DEF.path)).toBe('old');
	});

	it.each(['read', 'triage', 'none'])('refuses a user with %s access', async (role) => {
		const r = fakeRepo({ role });
		const err = await createProposalBranch(r.api, scope, 'issue').catch((e) => e);
		expect(err).toBeInstanceOf(RepoError);
		expect(err.status).toBe(403);
		expect(r.branches.size).toBe(1);
	});

	it.each(['admin', 'maintain'])('accepts %s', async (role) => {
		await expect(
			createProposalBranch(fakeRepo({ role }).api, scope, 'issue')
		).resolves.toBeTruthy();
	});

	it('commits only on the branch of a valid token for the same user', async () => {
		const r = fakeRepo();
		const { token } = await createProposalBranch(r.api, scope, 'issue');
		await expect(
			commitProposal(r.api, { ...scope, login: 'mallory' }, token, 'issue', [DEF])
		).rejects.toThrow(/invalid/);
		await expect(commitProposal(r.api, scope, 'forged.token', 'issue', [DEF])).rejects.toThrow(
			/invalid/
		);
		const mainToken = signProposal(scope.secret, {
			...scope,
			branch: 'main',
			base: 'main',
			baseSha: 'base0',
			exp: Date.now() + 60_000
		});
		await expect(commitProposal(r.api, scope, mainToken, 'issue', [DEF])).rejects.toThrow(
			/invalid/
		);
		expect(r.commits).toEqual([]);
	});

	it('refuses to commit when the branch moved since the flow made it', async () => {
		const r = fakeRepo();
		const { branch, token } = await createProposalBranch(r.api, scope, 'issue');
		r.branches.get(branch)!.sha = 'someone-else';
		const err = await commitProposal(r.api, scope, token, 'issue', [DEF]).catch((e) => e);
		expect(err.status).toBe(409);
		expect(r.commits).toEqual([]);
	});

	it('allows a second commit only on top of its own commit', async () => {
		const r = fakeRepo();
		const { token } = await createProposalBranch(r.api, scope, 'issue');
		const c1 = await commitProposal(r.api, scope, token, 'issue', [DEF]);
		await expect(commitProposal(r.api, scope, token, 'issue', [DEF])).rejects.toThrow(/changed/);
		await expect(commitProposal(r.api, scope, c1.token, 'issue', [PROMPT])).resolves.toBeTruthy();
	});

	it('opens no PR before a commit, or when the branch has other changes', async () => {
		const r = fakeRepo();
		const { branch, token } = await createProposalBranch(r.api, scope, 'issue');
		await expect(openProposal(r.api, scope, token, 'issue')).rejects.toThrow(/Nothing/);
		const c = await commitProposal(r.api, scope, token, 'issue', [DEF]);
		r.api.changedPaths = async () => [DEF.path, '.github/workflows/ci.yaml'];
		await expect(openProposal(r.api, scope, c.token, 'issue')).rejects.toThrow(/unexpected/);
		expect(r.prs).toEqual([]);
		expect(branch).toBeTruthy();
	});

	it.each(['bad\nname', 'x\u0007', '', '-lead', 'a'.repeat(65), 'Title: <script>'])(
		'rejects the workflow name %j',
		async (name) => {
			const r = fakeRepo();
			await expect(createProposalBranch(r.api, scope, name)).rejects.toThrow(/workflow name/);
			expect(r.branches.size).toBe(1);
		}
	);

	it('checks paths and sizes before anything else', () => {
		for (const path of [
			'.github/workflows/ci.yaml',
			'.enzarb/prompts/../../x.md',
			'.enzarb/prompts/a.b.md',
			'.enzarb/workflows/a/b.yaml',
			'README.md'
		])
			expect(() => checkFiles([{ path, text: 'x' }])).toThrow(/Only workflow definitions/);
		expect(() => checkFiles([])).toThrow(/Nothing/);
		const big = 'x'.repeat(200 * 1024);
		expect(() => checkFiles([{ path: DEF.path, text: 'é'.repeat(130 * 1024) }])).toThrow(
			/too large/
		);
		const many = Array.from({ length: 6 }, (_, i) => ({
			path: `.enzarb/prompts/p${i}.md`,
			text: big
		}));
		expect(() => checkFiles(many)).toThrow(/change is too large/);
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

	it('commits several files in one commit on top of the given head', async () => {
		const calls: [string, string, unknown][] = [];
		const f = vi.fn(async (url: string, init: RequestInit) => {
			const path = url.replace('https://api.github.com/repos/o/r', '');
			calls.push([init.method!, path, init.body ? JSON.parse(init.body as string) : undefined]);
			if (path === '/git/commits/head') return json({ tree: { sha: 'tree0' } });
			if (path === '/git/trees') return json({ sha: 'tree1' }, 201);
			if (path === '/git/commits') return json({ sha: 'new' }, 201);
			if (path.startsWith('/git/refs/heads/')) return json({});
			return json({}, 404);
		});
		const api = githubRepoApi('tok', 'o/r', f as unknown as typeof fetch);
		expect(await api.commitFiles('enzarb/workflow-x-abcdef', 'head', [DEF], 'msg')).toBe('new');
		expect(calls.map(([m, p]) => `${m} ${p}`)).toEqual([
			'GET /git/commits/head',
			'POST /git/trees',
			'POST /git/commits',
			'PATCH /git/refs/heads/enzarb/workflow-x-abcdef'
		]);
		expect(calls[1][2]).toEqual({
			base_tree: 'tree0',
			tree: [{ path: DEF.path, mode: '100644', type: 'blob', content: DEF.text }]
		});
		expect(calls[2][2]).toEqual({ message: 'msg', tree: 'tree1', parents: ['head'] });
		expect(calls[3][2]).toEqual({ sha: 'new', force: false });
		const headers = (f.mock.calls[0][1] as RequestInit).headers as Record<string, string>;
		expect(headers.Authorization).toBe('Bearer tok');
	});

	it('keeps GitHub detail out of the message', async () => {
		const api = githubRepoApi('t', 'o/r', (async () =>
			json({ message: 'secret internals' }, 500)) as unknown as typeof fetch);
		const err = await api.defaultBranch().catch((e) => e);
		expect(err.message).toBe('GitHub could not read the repository.');
		expect(err.detail).toContain('secret internals');
	});

	it('lists changed paths from compare', async () => {
		const f = vi.fn(async () => json({ files: [{ filename: 'a' }, { filename: 'b' }] }));
		const api = githubRepoApi('t', 'o/r', f as unknown as typeof fetch);
		expect(await api.changedPaths('x', 'y')).toEqual(['a', 'b']);
		expect((f.mock.calls[0] as unknown as [string])[0]).toBe(
			'https://api.github.com/repos/o/r/compare/x...y'
		);
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
