import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { ChatItem } from '#lib/acp/types.ts';
import ChatTranscript from './ChatTranscript.svelte';

const items: ChatItem[] = [
	{ kind: 'user', id: 'i1', text: 'hello' },
	{ kind: 'agent', id: 'i2', text: '<img src=x onerror="window.__pwned=1"> hi' },
	{ kind: 'tool', id: 'i3', toolCallId: 't', title: 'Run ls', status: 'completed', text: 'a.txt' },
	{
		kind: 'plan',
		id: 'i4',
		entries: [{ content: 'step one', status: 'pending', priority: 'high' }]
	},
	{
		kind: 'notice',
		id: 'i5',
		code: 'not_restored',
		text: 'Previous conversation could not be restored'
	},
	{
		kind: 'permission',
		id: 'perm-1',
		toolCall: { toolCallId: 't2', title: 'Run rm', text: '' },
		options: [{ optionId: 'a', name: 'Allow', kind: 'allow_once' }],
		status: 'pending'
	}
];

describe('ChatTranscript', () => {
	it('is a polite live region log', async () => {
		render(ChatTranscript, { items, epoch: 1, onAnswer: vi.fn() });
		const log = page.getByRole('log');
		await expect.element(log).toHaveAttribute('aria-live', 'polite');
	});

	it('renders every item kind', async () => {
		render(ChatTranscript, { items, epoch: 1, onAnswer: vi.fn() });
		await expect.element(page.getByText('hello')).toBeVisible();
		await expect.element(page.getByText('Run ls')).toBeVisible();
		await expect.element(page.getByText('a.txt')).toBeVisible();
		await expect.element(page.getByText(/step one/)).toBeVisible();
		await expect
			.element(page.getByText('Previous conversation could not be restored'))
			.toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Allow once' })).toBeVisible();
	});

	it('shows HTML in server text escaped, never as markup', async () => {
		render(ChatTranscript, { items, epoch: 1, onAnswer: vi.fn() });
		await expect.element(page.getByText(/<img src=x/)).toBeVisible();
		expect(document.querySelector('.ez-chat-log img')).toBeNull();
		expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
	});

	it('passes answers through with the epoch', async () => {
		const onAnswer = vi.fn();
		render(ChatTranscript, { items, epoch: 7, onAnswer });
		await page.getByRole('button', { name: 'Allow once' }).click();
		expect(onAnswer).toHaveBeenCalledWith('perm-1', 'allow', 7);
	});

	it('removes the buttons when the item is cancelled', async () => {
		const view = render(ChatTranscript, { items, epoch: 1, onAnswer: vi.fn() });
		const cancelled = items.map((i) =>
			i.kind === 'permission' ? { ...i, status: 'cancelled' as const } : i
		);
		await view.rerender({ items: cancelled, epoch: 2, onAnswer: vi.fn() });
		expect(page.getByRole('button').elements()).toHaveLength(0);
	});

	it('collapses the hidden context of a user message and keeps the user text visible', async () => {
		const hideContext = (t: string) =>
			t.startsWith('CTX|') ? { context: 'the secret rules', rest: t.slice(4) } : null;
		render(ChatTranscript, {
			items: [{ kind: 'user', id: 'u', text: 'CTX|add a rule' }],
			epoch: 1,
			onAnswer: vi.fn(),
			hideContext
		});
		await expect.element(page.getByText('add a rule')).toBeVisible();
		await expect.element(page.getByText('Context: current rules and file formats')).toBeVisible();
		await expect.element(page.getByText('the secret rules')).not.toBeVisible();
	});
});
