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
const WT = '/workspace/worktrees/chat-1/';
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

function setup(files: Record<string, string>) {
	const ch = fakeAgentChannel({
		sessionId: 's-1',
		onMessage(m: Msg, c) {
			if (m.method === 'session/prompt') {
				// The agent edits the rules file, then ends the turn.
				for (const update of [
					{
						sessionUpdate: 'tool_call',
						toolCallId: 'e1',
						title: 'Edit',
						kind: 'edit',
						status: 'pending'
					},
					{
						sessionUpdate: 'tool_call_update',
						toolCallId: 'e1',
						status: 'completed',
						content: [{ type: 'diff', path: `${WT}.ez/workflows.toml`, newText: 'x' }]
					}
				]) {
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
		features: new Set(['acp', 'file_read']),
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
		parseModes: vi.fn(async () => ({
			ok: true as const,
			doc: { ...doc, modes: [mode, { ...mode, id: 'reviewer' }] },
			issues: []
		})),
		checkRules: vi.fn(async () => ({ ok: true as const, diagnostics: [], file: added }))
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
			[`${WT}.ez/workflows.toml`]: 'version = 1\n# new',
			[`${WT}.ez/modes.toml`]: '[modes.reviewer]\n'
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

	it('discard dismisses the card and asks the agent to revert', async () => {
		sessionStorage.clear();
		const t = setup({ [`${WT}.ez/workflows.toml`]: 'x', [`${WT}.ez/modes.toml`]: 'y' });
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
});
