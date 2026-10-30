import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

vi.mock('$app/state', () => ({ page: { url: new URL('http://localhost/projects') } }));

import Nav from './Nav.svelte';

describe('Nav', () => {
	it('links to Projects and marks it current', async () => {
		render(Nav);
		await expect
			.element(page.getByRole('link', { name: 'Projects' }))
			.toHaveAttribute('aria-current', 'page');
	});
	it('links to Tasks and shows the count only when above zero', async () => {
		const { unmount } = render(Nav, { count: 0 });
		await expect
			.element(page.getByRole('link', { name: 'Tasks' }))
			.toHaveAttribute('href', '/tasks');
		unmount();
		render(Nav, { count: 3 });
		await expect.element(page.getByRole('link', { name: 'Tasks, 3 waiting' })).toBeVisible();
	});
	it('has no Settings link; it lives in the account menu', async () => {
		render(Nav);
		await expect.element(page.getByRole('link', { name: 'Settings' })).not.toBeInTheDocument();
	});
});
