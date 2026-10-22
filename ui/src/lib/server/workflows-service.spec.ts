import { beforeEach, describe, expect, it, vi } from 'vitest';
import { TracksError, type TracksClient } from './tracks.ts';
import {
	DEFAULT_WORKFLOWS_SUBJECT,
	dryRunWorkflows,
	loadWorkflows,
	saveWorkflows,
	type DryRunDeps,
	type WorkflowsDeps
} from './workflows-service.ts';

const project = {
	repositories: [
		{ url: 'https://github.com/Acme/Widgets', connector: 'github', defaultBranch: 'main' }
	]
};
const actor = { login: 'octo', githubId: 42 };
const b64 = (s: string) => Buffer.from(s).toString('base64');
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });
const blob = (text: string, sha: string) =>
	json(200, { type: 'file', encoding: 'base64', content: b64(text), sha, size: text.length });
const err = { severity: 'error', path: 'workflows.a.start', message: 'unknown node' } as const;

let gh: ReturnType<typeof vi.fn>;
let tracks: { [K in keyof TracksClient]: ReturnType<typeof vi.fn> };
let deps: WorkflowsDeps;
let files: Record<string, { text: string; sha: string } | null>;

beforeEach(() => {
	files = {
		'.ez/workflows.toml': { text: 'version = 1\n', sha: 'w1' },
		'.ez/modes.toml': { text: '[modes.plan]\n', sha: 'm1' }
	};
	gh = vi.fn(async (url: string, init: RequestInit) => {
		if (init.method === 'PUT')
			return json(200, {
				content: { sha: 'w2' },
				commit: { sha: 'c', html_url: 'https://github.com/c' }
			});
		const path = Object.keys(files).find((p) => url.includes(`/contents/${p}?`));
		const f = path ? files[path] : null;
		return f ? blob(f.text, f.sha) : json(404, {});
	});
	tracks = {
		modesCatalog: vi.fn(),
		parseModes: vi.fn(async () => ({
			doc: { defaultMode: 'plan', modes: [{ id: 'plan' }] },
			issues: []
		})),
		validateModes: vi.fn(),
		renderModes: vi.fn(),
		checkWorkflows: vi.fn(async () => ({ diagnostics: [], file: { version: 1 } })),
		renderWorkflows: vi.fn(async () => ({ text: 'version = 1\n# new\n', diagnostics: [] })),
		dryRun: vi.fn(async () => ({ report: { diagnostics: [] }, results: [] }))
	};
	deps = {
		tracks: tracks as unknown as TracksClient,
		checkAccess: vi.fn(async (_l: string, key: string) => ({
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

const put = () => gh.mock.calls.find(([, init]) => (init as RequestInit).method === 'PUT');

describe('loadWorkflows', () => {
	it('reads both files from the default branch and checks against the modes', async () => {
		const r = await loadWorkflows(deps, actor, project);
		expect(r).toMatchObject({
			ok: true,
			repo: 'github:acme/widgets',
			branch: 'trunk',
			sha: 'w1',
			text: 'version = 1\n',
			file: { version: 1 },
			modesToml: '[modes.plan]\n',
			modeIds: ['plan']
		});
		expect(tracks.checkWorkflows).toHaveBeenCalledWith('version = 1\n', '[modes.plan]\n');
		expect(gh.mock.calls.map(([u]) => u)).toEqual([
			'https://api.github.com/repos/acme/widgets/contents/.ez/workflows.toml?ref=trunk',
			'https://api.github.com/repos/acme/widgets/contents/.ez/modes.toml?ref=trunk'
		]);
	});

	it('opens a missing file as an empty document, and works without modes', async () => {
		files = { '.ez/workflows.toml': null, '.ez/modes.toml': null };
		expect(await loadWorkflows(deps, actor, project)).toMatchObject({
			ok: true,
			sha: null,
			text: null,
			file: { version: 1 },
			modesToml: null,
			modeIds: []
		});
		expect(tracks.checkWorkflows).not.toHaveBeenCalled();
		expect(tracks.parseModes).not.toHaveBeenCalled();
	});

	it('refuses users without write access and reports tracks being down', async () => {
		vi.mocked(deps.checkAccess).mockResolvedValueOnce(null);
		expect(await loadWorkflows(deps, actor, project)).toMatchObject({ code: 'forbidden' });
		tracks.checkWorkflows.mockRejectedValueOnce(new TracksError(503, 'unavailable', 'x'));
		expect(await loadWorkflows(deps, actor, project)).toMatchObject({ code: 'tracks' });
	});
});

describe('saveWorkflows', () => {
	const repo = 'github:acme/widgets';

	it('renders the document against the branch modes and commits it', async () => {
		const r = await saveWorkflows(deps, actor, project, {
			repo,
			baseSha: 'w1',
			file: { version: 1 }
		});
		expect(r).toMatchObject({
			ok: true,
			unchanged: false,
			sha: 'w2',
			text: 'version = 1\n# new\n'
		});
		expect(tracks.renderWorkflows).toHaveBeenCalledWith({ version: 1 }, '[modes.plan]\n');
		const body = JSON.parse((put()![1] as RequestInit).body as string);
		expect(body).toMatchObject({ branch: 'trunk', sha: 'w1' });
		expect(
			body.message.startsWith(
				`${DEFAULT_WORKFLOWS_SUBJECT}\n\nEdited in the enzarb workflow designer.`
			)
		).toBe(true);
		expect(body.message).toContain('Co-authored-by: octo');
	});

	it('never commits a file with errors (saving makes it live)', async () => {
		tracks.checkWorkflows.mockResolvedValueOnce({ diagnostics: [err], file: null });
		expect(
			await saveWorkflows(deps, actor, project, { repo, baseSha: 'w1', text: 'bad' })
		).toMatchObject({ ok: false, code: 'invalid', issues: [err] });
		tracks.renderWorkflows.mockResolvedValueOnce({ text: 'x', diagnostics: [err] });
		expect(
			await saveWorkflows(deps, actor, project, { repo, baseSha: 'w1', file: { version: 1 } })
		).toMatchObject({ ok: false, code: 'invalid' });
		expect(put()).toBeUndefined();
	});

	it('refuses a stale sha and skips unchanged text', async () => {
		expect(
			await saveWorkflows(deps, actor, project, { repo, baseSha: 'old', text: 'version = 1\n' })
		).toMatchObject({ ok: false, code: 'conflict' });
		expect(
			await saveWorkflows(deps, actor, project, { repo, baseSha: 'w1', text: 'version = 1\n' })
		).toMatchObject({ ok: true, unchanged: true });
		expect(put()).toBeUndefined();
	});
});

describe('dryRunWorkflows', () => {
	let dd: DryRunDeps;
	const ev = { id: 5, event: 'issues', action: 'labeled', payload: { x: 1 } };
	beforeEach(() => {
		dd = {
			tracks: tracks as unknown as TracksClient,
			events: vi.fn(async () => [ev])
		};
	});

	it("sends the repository's events in the chosen order and reports missing ones", async () => {
		const r = await dryRunWorkflows(dd, project, {
			repo: 'github:Acme/Widgets',
			text: 't',
			modesToml: null,
			eventIds: [9, 5, 5],
			options: { workflow: 'a' }
		});
		expect(r).toMatchObject({ ok: true, missing: [9] });
		expect(dd.events).toHaveBeenCalledWith('acme/widgets', [9, 5]);
		expect(tracks.dryRun).toHaveBeenCalledWith({
			text: 't',
			modes: null,
			events: [ev],
			options: { workflow: 'a' }
		});
	});

	it('refuses other repositories, empty selections and unknown events', async () => {
		const input = { text: 't', modesToml: null, eventIds: [5] };
		expect(await dryRunWorkflows(dd, project, { ...input, repo: 'github:evil/x' })).toMatchObject({
			code: 'no_repository'
		});
		expect(
			await dryRunWorkflows(dd, project, { ...input, repo: 'github:acme/widgets', eventIds: [] })
		).toMatchObject({ code: 'invalid' });
		vi.mocked(dd.events).mockResolvedValueOnce([]);
		expect(
			await dryRunWorkflows(dd, project, { ...input, repo: 'github:acme/widgets' })
		).toMatchObject({ code: 'invalid' });
		expect(tracks.dryRun).not.toHaveBeenCalled();
	});
});
