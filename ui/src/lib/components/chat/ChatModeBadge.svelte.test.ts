import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ChatModeBadge from './ChatModeBadge.svelte';

describe('ChatModeBadge', () => {
	it('spells out bypass in text and adds the modifier class', async () => {
		render(ChatModeBadge, { modeId: 'bypassPermissions' });
		await expect.element(page.getByText('Bypass – no permission prompts')).toBeVisible();
		expect(document.querySelector('.ez-chat-mode--bypass')).not.toBeNull();
	});

	it.each([
		['default', 'Default'],
		['plan', 'Plan']
	] as const)('shows %s as text without the bypass class', async (modeId, label) => {
		render(ChatModeBadge, { modeId });
		await expect.element(page.getByText(label)).toBeVisible();
		expect(document.querySelector('.ez-chat-mode--bypass')).toBeNull();
	});

	it("shows an ez mode's name", async () => {
		render(ChatModeBadge, { modeId: 'build', name: 'Build' });
		await expect.element(page.getByText('Build')).toBeVisible();
	});
});
