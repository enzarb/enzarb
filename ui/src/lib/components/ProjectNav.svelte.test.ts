import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ProjectNav from './ProjectNav.svelte';

describe('ProjectNav', () => {
	it('always shows the Chat tab', async () => {
		render(ProjectNav, { name: 'alpha', pathname: '/projects/alpha' });
		await expect.element(page.getByRole('link', { name: 'Terminal' })).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'Chat' }))
			.toHaveAttribute('href', '/projects/alpha/chat');
	});

	it('marks the Chat tab current on the chat route', async () => {
		render(ProjectNav, { name: 'alpha', pathname: '/projects/alpha/chat' });
		const link = page.getByRole('link', { name: 'Chat' });
		await expect.element(link).toHaveAttribute('href', '/projects/alpha/chat');
		await expect.element(link).toHaveAttribute('aria-current', 'page');
	});

	it('keeps the Events tab current and Overview off', async () => {
		render(ProjectNav, { name: 'alpha', pathname: '/projects/alpha/events' });
		await expect
			.element(page.getByRole('link', { name: 'Events' }))
			.toHaveAttribute('aria-current', 'page');
		await expect
			.element(page.getByRole('link', { name: 'Overview' }))
			.not.toHaveAttribute('aria-current');
	});
});
