import { expect, test } from '@playwright/test';
import { AGENT_REPLY, E2E_PROJECT } from './constants.ts';
import { installFakeSandbox } from './fake-sandbox.ts';

test('chat connects and exchanges a message', async ({ page }) => {
	await installFakeSandbox(page);
	await page.goto(`/projects/${E2E_PROJECT}/chat`);

	// The composer is disabled until the chat reached the connected (ready) state.
	const composer = page.getByRole('textbox', { name: 'Message' });
	await expect(composer).toBeEnabled();

	// The chat fills the viewport: no page scroll, composer pinned inside it.
	const noPageScroll = await page.evaluate(
		() => document.documentElement.scrollHeight <= window.innerHeight
	);
	expect(noPageScroll).toBe(true);
	await expect(composer).toBeInViewport();

	await composer.fill('Hello agent');
	await page.getByRole('button', { name: 'Send' }).click();

	const log = page.getByRole('log', { name: 'Conversation' });
	await expect(log).toContainText('Hello agent');
	await expect(log).toContainText(AGENT_REPLY);
});
