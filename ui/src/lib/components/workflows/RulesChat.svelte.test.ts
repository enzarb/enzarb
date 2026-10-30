import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { AcpChannel, AcpSession } from '#lib/acp/connection.ts';
import { fakeAgentChannel, type Msg } from '#lib/test/acp-fake.ts';
import { exampleWorkflow, toWire, type WorkflowsFile } from '#lib/workflows-types.ts';
import RulesChat from './RulesChat.svelte';
import type { TemplatesApi } from './RulesView.svelte';
import type { WorkflowsApi } from './WorkflowsDesigner.svelte';

const chatConnection = vi.hoisted(() => vi.fn(() => new Promise(() => {})));
vi.mock('#lib/chat.remote.ts', () => ({ chatConnection }));

const wire = toWire({ workflows: [exampleWorkflow('fix-issue')] });
const file: WorkflowsFile = {
	repo: 'github:acme/w',
	branch: 'main',
	path: '.ez/workflows.toml',
	sha: 'a'.repeat(40),
	text: 'version = 1\n',
	file: wire,
	report: [],
	modesToml: '[modes.triage]\n',
	modeIds: ['triage']
};
const mode = {
	id: 'triage',
	description: '',
	prompt: '',
	tools: null,
	miseTasks: [],
	mcpTools: [],
	next: [],
	autoAdvanceTo: [],
	permissions: {},
	hooks: {}
};
const doc = { defaultMode: 'triage', modes: [mode] };

function setup(files: Record<string, string>, edit = true) {
	const ch = fakeAgentChannel({
		sessionId: 's-1',
		onMessage(m: Msg, c) {
			if (m.method === 'session/prompt') {
				// The agent edits the rules by a shell command that names no .ez path, then ends the turn.
				for (const update of edit
					? [
							{
								sessionUpdate: 'tool_call',
								toolCallId: 'e1',
								title: 'sed -i s/a/b/ config',
								kind: 'execute',
								status: 'pending'
							},
							{
								sessionUpdate: 'tool_call_update',
								toolCallId: 'e1',
								status: 'completed',
								content: [{ type: 'content', content: { type: 'text', text: 'done' } }]
							}
						]
					: []) {
					c.send({
						jsonrpc: '2.0',
						method: 'session/update',
						params: { sessionId: 's-1', update }
					});
				}
				c.send({ jsonrpc: '2.0', id: m.id, result: { stopReason: 'end_turn' } });
				return true;
			}
		}
	});
	const readFile = vi.fn(async (path: string) =>
		path in files
			? { ok: true as const, path, content: files[path] }
			: { ok: false as const, path, code: 'not_found', message: 'missing' }
	);
	const s: AcpSession = {
		features: new Set(['acp', 'file_read', 'file_read_chat']),
		openAcp: vi.fn(async () => ch as unknown as AcpChannel),
		readFile
	};
	const calls: string[] = [];
	const api: WorkflowsApi = {
		render: vi.fn(async () => ({ ok: true as const, text: 'x', diagnostics: [] })),
		check: vi.fn(),
		save: vi.fn(async () => {
			calls.push('rules');
			return { ok: true as const, unchanged: false, commitUrl: null };
		}),
		reload: vi.fn()
	} as unknown as WorkflowsApi;
	const templates: TemplatesApi = {
		load: vi.fn(async () => ({ doc, sha: 'm'.repeat(40), text: '[modes.triage]\n' })),
		renderModes: vi.fn(async () => ({ ok: true as const, text: 'rendered' })),
		saveModes: vi.fn(async () => {
			calls.push('modes');
			return { ok: true as const };
		})
	};
	const added = {
		...wire,
		workflows: { ...wire.workflows, review: { ...wire.workflows!['fix-issue'], on: ['push'] } }
	};
	const check = {
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		parseModes: vi.fn<() => Promise<any>>(async () => ({
			ok: true as const,
			doc: { ...doc, modes: [mode, { ...mode, id: 'reviewer' }] },
			issues: []
		})),
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		checkRules: vi.fn<() => Promise<any>>(async () => ({
			ok: true as const,
			diagnostics: [],
			file: added
		}))
	};
	const saved = vi.fn();
	const props = {
		project: 'alpha',
		file,
		api,
		templates,
		check,
		saved,
		highlighted: vi.fn(),
		connect: async () => s
	};
	return { ch, readFile, calls, templates, api, saved, props };
}

describe('RulesChat', () => {
	it('sends the context first, shows a proposal for an edit, and accepts modes first', async () => {
		sessionStorage.clear();
		const t = setup({
			['.ez/workflows.toml']: 'version = 1\n# new',
			['.ez/modes.toml']: '[modes.reviewer]\n'
		});
		render(RulesChat, t.props);
		const box = page.getByRole('textbox', { name: 'Message' });
		await expect.element(box).toBeEnabled();
		await box.fill('add a review rule');
		await page.getByRole('button', { name: 'Send' }).click();

		await expect.element(page.getByText(/Added the rule "review"/)).toBeVisible();
		await expect.element(page.getByText('Added the mode "reviewer"')).toBeVisible();
		const prompt = JSON.stringify(t.ch.received.find((m) => m.method === 'session/prompt'));
		expect(prompt).toContain('When <events>');
		expect(prompt).toContain('Never commit');
		expect(prompt).toContain('add a review rule');

		await page.getByRole('button', { name: 'Accept and commit' }).click();
		await vi.waitFor(() => expect(t.saved).toHaveBeenCalledOnce());
		expect(t.calls).toEqual(['modes', 'rules']);
		await expect.element(page.getByText(/Added the rule "review"/)).not.toBeInTheDocument();
	});

	it('reports a modes-only accept as a change', async () => {
		sessionStorage.clear();
		const t = setup({
			'.ez/workflows.toml': 'version = 1\n',
			'.ez/modes.toml': '[modes.reviewer]\n'
		});
		t.props.check.checkRules.mockImplementation(async () => ({
			ok: true as const,
			diagnostics: [],
			file: wire
		}));
		render(RulesChat, t.props);
		const box = page.getByRole('textbox', { name: 'Message' });
		await expect.element(box).toBeEnabled();
		await box.fill('add a reviewer mode');
		await page.getByRole('button', { name: 'Send' }).click();
		await page.getByRole('button', { name: 'Accept and commit' }).click();
		await vi.waitFor(() => expect(t.saved).toHaveBeenCalledOnce());
		expect(t.calls).toEqual(['modes']);
		expect(t.saved.mock.calls[0][0].unchanged).toBe(false);
	});

	it('proposes nothing when the files did not change', async () => {
		sessionStorage.clear();
		const t = setup({
			'.ez/workflows.toml': 'version = 1\n',
			'.ez/modes.toml': '[modes.triage]\n'
		});
		t.props.check.checkRules.mockImplementation(async () => ({
			ok: true as const,
			diagnostics: [],
			file: wire
		}));
		t.props.check.parseModes.mockImplementation(async () => ({
			ok: true as const,
			doc,
			issues: []
		}));
		render(RulesChat, t.props);
		const box = page.getByRole('textbox', { name: 'Message' });
		await expect.element(box).toBeEnabled();
		await box.fill('go');
		await page.getByRole('button', { name: 'Send' }).click();
		await vi.waitFor(() => expect(t.readFile).toHaveBeenCalled());
		await vi.waitFor(() => expect(t.props.check.checkRules).toHaveBeenCalledTimes(1));
		await expect
			.element(page.getByRole('button', { name: 'Accept and commit' }))
			.not.toBeInTheDocument();
		// Re-reads after the tool call and the end of the turn validate the same content once.
		expect(t.props.check.checkRules).toHaveBeenCalledTimes(1);
	});

	it('re-reads at the end of a turn even when no tool call ran', async () => {
		sessionStorage.clear();
		const t = setup({ '.ez/workflows.toml': 'x', '.ez/modes.toml': 'y' }, false);
		render(RulesChat, t.props);
		const box = page.getByRole('textbox', { name: 'Message' });
		await expect.element(box).toBeEnabled();
		await box.fill('go');
		await page.getByRole('button', { name: 'Send' }).click();
		await expect.element(page.getByText(/Added the rule "review"/)).toBeVisible();
	});

	it('discard dismisses the card and asks the agent to revert', async () => {
		sessionStorage.clear();
		const t = setup({ ['.ez/workflows.toml']: 'x', ['.ez/modes.toml']: 'y' });
		render(RulesChat, t.props);
		const box = page.getByRole('textbox', { name: 'Message' });
		await expect.element(box).toBeEnabled();
		await box.fill('go');
		await page.getByRole('button', { name: 'Send' }).click();
		await page.getByRole('button', { name: 'Discard' }).click();
		await expect.element(page.getByText(/Added the rule "review"/)).not.toBeInTheDocument();
		await vi.waitFor(() =>
			expect(
				t.ch.received
					.filter((m) => m.method === 'session/prompt')
					.some((m) => JSON.stringify(m).includes('Revert your edits'))
			).toBe(true)
		);
		expect(t.calls).toEqual([]);
	});

	it('hides the context in the transcript but keeps the user text', async () => {
		sessionStorage.clear();
		const t = setup({ '.ez/workflows.toml': 'x', '.ez/modes.toml': 'y' });
		render(RulesChat, t.props);
		const box = page.getByRole('textbox', { name: 'Message' });
		await expect.element(box).toBeEnabled();
		await box.fill('add a review rule');
		await page.getByRole('button', { name: 'Send' }).click();
		await expect.element(page.getByText('Context: current rules and file formats')).toBeVisible();
		await expect.element(page.getByText('add a review rule', { exact: true })).toBeVisible();
		await expect.element(page.getByText('When <events>')).not.toBeVisible();
	});

	it('asks the agent once, automatically, to fix invalid files, and offers a button', async () => {
		sessionStorage.clear();
		const t = setup({ '.ez/workflows.toml': 'bad', '.ez/modes.toml': 'y' });
		t.props.check.checkRules.mockImplementation(async () => ({
			ok: true as const,
			diagnostics: [{ severity: 'error', message: 'invalid type: sequence' }],
			file: null
		}));
		render(RulesChat, t.props);
		const box = page.getByRole('textbox', { name: 'Message' });
		await expect.element(box).toBeEnabled();
		await box.fill('go');
		await page.getByRole('button', { name: 'Send' }).click();
		const fixes = () =>
			t.ch.received
				.filter((m) => m.method === 'session/prompt')
				.filter((m) => JSON.stringify(m).includes('Your last edit')).length;
		await vi.waitFor(() => expect(fixes()).toBe(1));
		await expect.element(page.getByText(/invalid type: sequence/).first()).toBeVisible();
		// the same errors after the fix turn do not trigger another automatic request
		await new Promise((r) => setTimeout(r, 300));
		expect(fixes()).toBe(1);
		await page.getByRole('button', { name: 'Ask the agent to fix it' }).click();
		await vi.waitFor(() => expect(fixes()).toBe(2));
	});
});
