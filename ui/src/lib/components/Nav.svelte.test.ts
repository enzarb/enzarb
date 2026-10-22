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
	it('has no Settings link; it lives in the account menu', async () => {
		render(Nav);
		await expect.element(page.getByRole('link', { name: 'Settings' })).not.toBeInTheDocument();
	});
});
