import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { AboutComponent } from '#lib/about-types.ts';
import ComponentVersionCard from './ComponentVersionCard.svelte';

const rel = (tag: string) => ({
	tag,
	name: `Release ${tag}`,
	publishedAt: '2026-10-01T00:00:00Z',
	url: `https://github.com/enzarb/enzarb/releases/tag/ui/${tag}`,
	html: `<p>notes ${tag}</p>`
});
const base: AboutComponent = {
	name: 'enzarb-ui',
	label: 'UI',
	repoUrl: 'https://github.com/enzarb/enzarb/tree/main/ui',
	deployed: 'v1.1.0',
	releases: [rel('v1.2.0'), rel('v1.1.0'), rel('v1.0.0')]
};

describe('ComponentVersionCard', () => {
	it('opens only the deployed release, newest first, with release links', async () => {
		render(ComponentVersionCard, { component: base });
		const details = document.querySelectorAll('details');
		expect([...details].map((d) => d.open)).toEqual([false, true, false]);
		expect([...details].map((d) => d.querySelector('.ez-release__name')?.textContent)).toEqual([
			'Release v1.2.0',
			'Release v1.1.0',
			'Release v1.0.0'
		]);
		await expect.element(page.getByText('v1.1.0', { exact: true }).first()).toBeVisible();
		expect(document.querySelector('a[href$="releases/tag/ui/v1.1.0"]')).not.toBeNull();
	});

	it('says there is no matching release and collapses everything', async () => {
		render(ComponentVersionCard, { component: { ...base, deployed: 'main-abc1234' } });
		await expect.element(page.getByText(/No matching release/)).toBeVisible();
		expect([...document.querySelectorAll('details')].some((d) => d.open)).toBe(false);
	});

	it('shows unknown without a no-match warning', async () => {
		render(ComponentVersionCard, { component: { ...base, deployed: 'unknown' } });
		await expect.element(page.getByText('unknown', { exact: true })).toBeVisible();
		expect(document.body.textContent).not.toMatch(/No matching release/);
	});

	it('shows the version when release notes are unavailable', async () => {
		render(ComponentVersionCard, { component: { ...base, releases: null } });
		await expect.element(page.getByText('Release notes unavailable.')).toBeVisible();
		await expect.element(page.getByText('v1.1.0', { exact: true })).toBeVisible();
	});
});
