import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { completeChatStep, failChatStep, initialChatProgress } from '#lib/chat-steps.ts';
import ChatStatus from './ChatStatus.svelte';

const atAgent = ['token', 'connect', 'handshake'].reduce(
	(p, id) => completeChatStep(p, id as 'token'),
	initialChatProgress
);

describe('ChatStatus', () => {
	it('lists the steps with their states', async () => {
		render(ChatStatus, { progress: atAgent, onAction: vi.fn() });
		const items = page.getByRole('listitem');
		await expect.element(items.nth(0)).toHaveAttribute('data-state', 'done');
		await expect.element(items.nth(3)).toHaveAttribute('data-state', 'active');
		await expect.element(items.nth(3)).toHaveTextContent('Starting agent…');
		await expect.element(items.nth(5)).toHaveAttribute('data-state', 'pending');
	});

	it('links to the Claude setup for a missing token instead of offering a retry', async () => {
		const onAction = vi.fn();
		render(ChatStatus, { progress: atAgent, action: 'set_up', onAction });
		await expect
			.element(page.getByRole('link', { name: 'Set up your Claude token' }))
			.toHaveAttribute('href', '/settings#claude');
		await expect.element(page.getByRole('button')).not.toBeInTheDocument();
	});

	it('shows server-derived failure text escaped', async () => {
		const progress = failChatStep(atAgent, '<b>boom</b>');
		render(ChatStatus, { progress, onAction: vi.fn() });
		await expect.element(page.getByRole('listitem').nth(3)).toHaveTextContent('<b>boom</b>');
		expect(document.querySelector('.ez-steps b')).toBeNull();
	});

	it.each([
		['take_over', 'Take over'],
		['reconnect', 'Reconnect'],
		['retry', 'Retry']
	] as const)('%s button calls back', async (action, label) => {
		const onAction = vi.fn();
		render(ChatStatus, {
			progress: atAgent,
			message: 'Chat opened in another tab',
			action,
			onAction
		});
		await expect.element(page.getByRole('status')).toHaveTextContent('Chat opened in another tab');
		await page.getByRole('button', { name: label }).click();
		expect(onAction).toHaveBeenCalledWith(action);
	});

	it('has no button without an action', async () => {
		render(ChatStatus, { progress: atAgent, onAction: vi.fn() });
		expect(page.getByRole('button').elements()).toHaveLength(0);
	});
});
