import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { ReleaseTiming } from '#lib/about-types.ts';
import ReleaseTimings from './ReleaseTimings.svelte';

const full: ReleaseTiming = {
	version: 'v0.9.0',
	url: 'https://github.com/enzarb/enzarb/releases/tag/helm/v0.9.0',
	publishedAt: '2026-10-01T10:00:00Z',
	deployedAt: '2026-10-01T10:03:00.000Z',
	sandboxAppliedAt: '2026-10-01T10:20:00.000Z',
	mergeToDeployedSeconds: 180,
	skew: false,
	deployToSandboxSeconds: 1020
};

describe('ReleaseTimings', () => {
	it('shows merge-to-deployed and the time to the sandbox', async () => {
		render(ReleaseTimings, { timings: [full] });
		await expect.element(page.getByText('Recent releases')).toBeVisible();
		await expect.element(page.getByText('(3m 00s after merge)')).toBeVisible();
		await expect.element(page.getByText('(17m 00s after deploy)')).toBeVisible();
	});

	it('shows a dash for each missing stage and no error', async () => {
		render(ReleaseTimings, {
			timings: [
				{
					...full,
					deployedAt: null,
					sandboxAppliedAt: null,
					mergeToDeployedSeconds: null,
					deployToSandboxSeconds: null
				}
			]
		});
		const cells = [...document.querySelectorAll('tbody td')].map((c) => c.textContent?.trim());
		expect(cells.slice(2)).toEqual(['—', '—']);
		expect(document.body.textContent).not.toMatch(/error|failed/i);
	});

	it('says n/a for clock skew and renders nothing for an empty list', async () => {
		const view = render(ReleaseTimings, {
			timings: [{ ...full, mergeToDeployedSeconds: null, skew: true }]
		});
		await expect.element(page.getByText('(n/a)')).toBeVisible();
		view.unmount();
		render(ReleaseTimings, { timings: [] });
		expect(document.querySelector('table')).toBeNull();
	});
});
