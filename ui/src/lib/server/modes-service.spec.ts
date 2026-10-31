import { beforeEach, describe, expect, it, vi } from 'vitest';
import { blankMode, type ModesDoc } from '#lib/modes-types.ts';
import {
	DEFAULT_COMMIT_SUBJECT,
	loadModes,
	projectRepoKeys,
	saveModes,
	type ModesDeps
} from './modes-service.ts';
import { TracksError, type TracksClient } from './tracks.ts';

const project = {
	repositories: [
		{ url: 'https://github.com/Acme/Widgets', connector: 'github', defaultBranch: 'main' },
		{ url: 'https://github.com/acme/other.git', connector: 'github', defaultBranch: 'main' }
	]
};
const actor = { login: 'octo', githubId: 42 };
const doc: ModesDoc = { defaultMode: 'plan', modes: [blankMode('plan')] };
const catalog = {
	builtinTools: [],
	hookEvents: [],
	permissionValues: [],
	onFailureValues: [],
	templates: []
};
const b64 = (s: string) => Buffer.from(s).toString('base64');
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });

let gh: ReturnType<typeof vi.fn>;
let tracks: { [K in keyof TracksClient]: ReturnType<typeof vi.fn> };
let deps: ModesDeps;
let file: { text: string; sha: string } | null;

beforeEach(() => {
	file = { text: '[modes.plan]\n', sha: 'sha1' };
	gh = vi.fn(async (url: string, init: RequestInit) => {
		if (init.method === 'PUT')
			return json(200, {
				content: { sha: 'sha2' },
				commit: { sha: 'c', html_url: 'https://github.com/c' }
			});
		if (url.includes('/contents/'))
			return file
				? json(200, {
						type: 'file',
						encoding: 'base64',
						content: b64(file.text),
						sha: file.sha,
						size: 10
					})
				: json(404, {});
		return json(500, {});
	});
	tracks = {
		decisions: vi.fn(async () => []),
		runs: vi.fn(async () => []),
		run: vi.fn(async () => null),
		stopRun: vi.fn(async () => false),
		resumeRun: vi.fn(async () => ({ result: 'ok' as const })),
		startRun: vi.fn(async () => ({ id: null, alreadyRunning: false })),
		modesCatalog: vi.fn(async () => catalog),
		parseModes: vi.fn(async () => ({ doc, issues: [] })),
		validateModes: vi.fn(async () => []),
		renderModes: vi.fn(async () => ({
			text: '[modes.plan]\nprompt = "x"\n',
			issues: [],
			rewritten: []
		})),
		checkWorkflows: vi.fn(),
		renderWorkflows: vi.fn(),
		dryRun: vi.fn()
	};
	deps = {
		tracks: tracks as unknown as TracksClient,
		checkAccess: vi.fn(async (_login: string, key: string) => ({
			repo: key,
			installationId: 1,
			accountLogin: 'acme',
			private: false,
			defaultBranch: 'trunk'
		})),
		token: vi.fn(async () => 'ghs_token'),
		invalidate: vi.fn(),
		fetch: gh as unknown as typeof fetch
	};
});

describe('projectRepoKeys', () => {
	it('lowercases GitHub URLs and strips .git', () => {
		expect(projectRepoKeys(project)).toEqual(['github:acme/widgets', 'github:acme/other']);
	});
});

describe('loadModes', () => {
	it('loads the first repository from its live default branch', async () => {
		const r = await loadModes(deps, actor, project);
		expect(r).toMatchObject({
			ok: true,
			repo: 'github:acme/widgets',
			branch: 'trunk',
			sha: 'sha1',
			doc,
			parseError: null
		});
		expect(deps.checkAccess).toHaveBeenCalledWith('octo', 'github:acme/widgets');
		expect(gh.mock.calls[0][0]).toBe(
			'https://api.github.com/repos/acme/widgets/contents/.ez/modes.toml?ref=trunk'
		);
		expect(deps.token).toHaveBeenCalledWith('acme/widgets', { contents: 'read' });
	});

	it('refuses repositories outside the project and users without write access', async () => {
		expect(await loadModes(deps, actor, project, 'github:evil/repo')).toMatchObject({
			ok: false,
			code: 'no_repository'
		});
		vi.mocked(deps.checkAccess).mockResolvedValueOnce(null);
		expect(await loadModes(deps, actor, project, 'github:acme/other')).toMatchObject({
			ok: false,
			code: 'forbidden'
		});
		expect(gh).not.toHaveBeenCalled();
	});

	it('returns an empty document for a missing file and the parse error for a broken one', async () => {
		file = null;
		expect(await loadModes(deps, actor, project)).toMatchObject({
			ok: true,
			sha: null,
			text: null,
			doc: { modes: [] }
		});
		file = { text: 'nope =', sha: 's' };
		tracks.parseModes.mockRejectedValueOnce(
			new TracksError(422, 'invalid_modes_toml', 'parsing modes.toml')
		);
		expect(await loadModes(deps, actor, project)).toMatchObject({
			ok: true,
			text: 'nope =',
			parseError: 'parsing modes.toml'
		});
	});

	it('reports tracks being down', async () => {
		tracks.modesCatalog.mockRejectedValueOnce(new TracksError(503, 'unavailable', 'x'));
		expect(await loadModes(deps, actor, project)).toMatchObject({ ok: false, code: 'tracks' });
	});
});

describe('saveModes', () => {
	const put = () => gh.mock.calls.find(([, init]) => (init as RequestInit).method === 'PUT');

	it('renders the doc onto the current file and commits with the co-author trailer', async () => {
		const r = await saveModes(deps, actor, project, {
			repo: 'github:acme/widgets',
			baseSha: 'sha1',
			doc
		});
		expect(r).toMatchObject({
			ok: true,
			unchanged: false,
			sha: 'sha2',
			commitUrl: 'https://github.com/c',
			text: '[modes.plan]\nprompt = "x"\n'
		});
		// Set from the commit itself: the new blob sha and text, not a read back from GitHub.
		expect(r).toMatchObject({
			loaded: { ok: true, sha: 'sha2', text: '[modes.plan]\nprompt = "x"\n' }
		});
		expect(tracks.renderModes).toHaveBeenCalledWith(doc, '[modes.plan]\n');
		const body = JSON.parse((put()![1] as RequestInit).body as string);
		expect(body).toMatchObject({ branch: 'trunk', sha: 'sha1' });
		expect(body.message).toMatch(
			new RegExp(`^${DEFAULT_COMMIT_SUBJECT.replace(/[().]/g, '\\$&')}\n`)
		);
		expect(body.message).toContain('Co-authored-by: octo <42+octo@users.noreply.github.com>');
		expect(deps.token).toHaveBeenCalledWith('acme/widgets', { contents: 'write' });
	});

	it('refuses a stale base sha before rendering or committing', async () => {
		const r = await saveModes(deps, actor, project, {
			repo: 'github:acme/widgets',
			baseSha: 'old',
			doc
		});
		expect(r).toMatchObject({ ok: false, code: 'conflict' });
		expect(tracks.renderModes).not.toHaveBeenCalled();
		expect(put()).toBeUndefined();
	});

	it('re-checks write access on every save', async () => {
		vi.mocked(deps.checkAccess).mockResolvedValueOnce(null);
		expect(
			await saveModes(deps, actor, project, { repo: 'github:acme/widgets', baseSha: 'sha1', doc })
		).toMatchObject({ ok: false, code: 'forbidden' });
		expect(put()).toBeUndefined();
	});

	it('returns the issues of an invalid doc or text and commits nothing', async () => {
		const issues = [{ path: 'defaultMode', message: 'unknown mode', severity: 'error' }];
		tracks.renderModes.mockRejectedValueOnce(
			new TracksError(422, 'invalid_modes', 'x', issues as never)
		);
		expect(
			await saveModes(deps, actor, project, { repo: 'github:acme/widgets', baseSha: 'sha1', doc })
		).toMatchObject({ ok: false, code: 'invalid', issues });
		tracks.parseModes.mockResolvedValueOnce({ doc, issues });
		expect(
			await saveModes(deps, actor, project, {
				repo: 'github:acme/widgets',
				baseSha: 'sha1',
				text: 'x'
			})
		).toMatchObject({ ok: false, code: 'invalid', issues });
		expect(put()).toBeUndefined();
	});

	it('commits raw text as-is, creates a missing file, and skips unchanged content', async () => {
		file = null;
		const r = await saveModes(deps, actor, project, {
			repo: 'github:acme/widgets',
			baseSha: null,
			text: '# mine\n',
			message: 'feat: modes'
		});
		expect(r).toMatchObject({ ok: true, unchanged: false });
		const body = JSON.parse((put()![1] as RequestInit).body as string);
		expect(Buffer.from(body.content, 'base64').toString()).toBe('# mine\n');
		expect(body).not.toHaveProperty('sha');
		expect(body.message.startsWith('feat: modes\n')).toBe(true);

		gh.mockClear();
		file = { text: '# same\n', sha: 's' };
		expect(
			await saveModes(deps, actor, project, {
				repo: 'github:acme/widgets',
				baseSha: 's',
				text: '# same\n'
			})
		).toMatchObject({ ok: true, unchanged: true });
		expect(put()).toBeUndefined();
	});

	it('maps GitHub refusals', async () => {
		gh.mockImplementation(async (url: string, init: RequestInit) =>
			init.method === 'PUT'
				? json(409, {})
				: json(200, { type: 'file', encoding: 'base64', content: b64('a'), sha: 'sha1', size: 1 })
		);
		expect(
			await saveModes(deps, actor, project, {
				repo: 'github:acme/widgets',
				baseSha: 'sha1',
				text: 'b'
			})
		).toMatchObject({ ok: false, code: 'conflict' });
		gh.mockImplementation(async (url: string, init: RequestInit) =>
			init.method === 'PUT'
				? json(403, {})
				: json(200, { type: 'file', encoding: 'base64', content: b64('a'), sha: 'sha1', size: 1 })
		);
		expect(
			await saveModes(deps, actor, project, {
				repo: 'github:acme/widgets',
				baseSha: 'sha1',
				text: 'b'
			})
		).toMatchObject({ ok: false, code: 'forbidden' });
	});
});
