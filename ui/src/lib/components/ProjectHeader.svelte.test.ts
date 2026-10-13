import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ProjectHeader from './ProjectHeader.svelte';

describe('ProjectHeader', () => {
	it('puts the title, subtext and tabs in one header row', async () => {
		render(ProjectHeader, {
			displayName: 'Alpha',
			name: 'alpha',
			sandboxName: 'sb',
			pathname: '/projects/alpha'
		});
		const headers = document.querySelectorAll('header.ez-project-header');
		expect(headers.length).toBe(1);
		const el = headers[0];
		expect(el.querySelector('h1')?.textContent).toBe('Alpha');
		expect(el.textContent).toContain('sandbox');
		expect(el.querySelector('nav[aria-label="Project sections"]')).not.toBeNull();
		await expect.element(page.getByRole('link', { name: 'Chat' })).toBeVisible();
	});
});
