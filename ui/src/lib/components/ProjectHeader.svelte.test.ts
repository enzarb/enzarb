import { page, userEvent } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
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

	const props = {
		displayName: 'Alpha',
		name: 'alpha',
		sandboxName: 'sb',
		pathname: '/projects/alpha'
	};
	const nameHint = /Kubernetes name, used in URLs and keys/;
	const sandboxHint = /sandbox pod that runs this Project's agents/;

	// A fresh headless page has no focus (document.hasFocus() is false), so a bare Tab does
	// nothing and these tests only passed when an earlier test had focused the page. Click the
	// heading first: it focuses the page and starts sequential navigation before the first stop.
	async function focusPage() {
		await userEvent.click(page.getByRole('heading', { name: 'Alpha' }));
	}

	it('shows the name tooltip on hover', async () => {
		render(ProjectHeader, props);
		await userEvent.hover(page.getByText('alpha', { exact: true }));
		await expect.element(page.getByRole('tooltip')).toHaveTextContent(nameHint);
	});

	it('shows on keyboard focus, hides on Escape and on blur', async () => {
		render(ProjectHeader, props);
		await focusPage();
		await userEvent.tab();
		await expect.element(page.getByRole('tooltip')).toHaveTextContent(nameHint);
		await userEvent.keyboard('{Escape}');
		await expect.poll(() => document.querySelector('[role="tooltip"]')).toBeNull();
		await userEvent.tab();
		await expect.element(page.getByRole('tooltip')).toHaveTextContent(sandboxHint);
		await userEvent.tab();
		await expect.poll(() => document.querySelector('[role="tooltip"]')).toBeNull();
	});

	it('links the trigger to the tooltip with aria-describedby', async () => {
		render(ProjectHeader, props);
		await focusPage();
		await userEvent.tab();
		const tip = await vi.waitUntil(() => document.querySelector('[role="tooltip"]'));
		const trigger = document.activeElement;
		expect(tip.id).not.toBe('');
		expect(trigger?.getAttribute('aria-describedby')).toBe(tip.id);
	});

	it('explains the sandbox label and name with one tooltip and tab stop', async () => {
		render(ProjectHeader, props);
		expect(document.querySelectorAll('.ez-project-header__meta [tabindex="0"]').length).toBe(2);
		await userEvent.hover(page.getByText('sb', { exact: true }));
		await expect.element(page.getByRole('tooltip')).toHaveTextContent(sandboxHint);
	});

	it.each([null, undefined])('omits the sandbox items when sandboxName is %s', (sandboxName) => {
		render(ProjectHeader, { ...props, sandboxName });
		expect(document.body.textContent).not.toContain('sandbox');
		expect(document.querySelectorAll('.ez-project-header__meta [tabindex="0"]').length).toBe(1);
	});

	it('uses no native title attributes on the subtext', () => {
		render(ProjectHeader, props);
		expect(document.querySelectorAll('.ez-project-header__meta [title]').length).toBe(0);
	});
});
