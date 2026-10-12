import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ProjectNav from './ProjectNav.svelte';

describe('ProjectNav', () => {
	it('has no Chat tab when the flag is off (the default)', async () => {
		render(ProjectNav, { name: 'alpha', pathname: '/projects/alpha' });
		await expect.element(page.getByRole('link', { name: 'Terminal' })).toBeVisible();
		await expect.element(page.getByRole('link', { name: 'Chat' })).not.toBeInTheDocument();
	});

	it('shows the Chat tab when the flag is on', async () => {
		render(ProjectNav, { name: 'alpha', pathname: '/projects/alpha/chat', chat: true });
		const link = page.getByRole('link', { name: 'Chat' });
		await expect.element(link).toHaveAttribute('href', '/projects/alpha/chat');
		await expect.element(link).toHaveAttribute('aria-current', 'page');
	});
});
