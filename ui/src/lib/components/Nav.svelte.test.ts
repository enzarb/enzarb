import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

vi.mock('$app/state', () => ({ page: { url: new URL('http://localhost/projects') } }));

import Nav from './Nav.svelte';

describe('Nav', () => {
	it('shows no badge at zero', async () => {
		render(Nav, { needsYou: 0 });
		await expect.element(page.getByRole('img')).not.toBeInTheDocument();
	});
	it('shows an accessible count on the Work link', async () => {
		render(Nav, { needsYou: 3 });
		await expect.element(page.getByRole('img', { name: '3 items need you' })).toBeVisible();
	});
	it('uses the singular for one item', async () => {
		render(Nav, { needsYou: 1 });
		await expect.element(page.getByRole('img', { name: '1 item needs you' })).toBeVisible();
	});
});
