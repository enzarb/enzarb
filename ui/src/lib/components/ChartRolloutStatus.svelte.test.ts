import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { ChartRollout } from '#lib/about-types.ts';
import ChartRolloutStatus from './ChartRolloutStatus.svelte';

const base: ChartRollout = {
	installed: '0.9.0',
	desired: '0.9.0',
	latest: '0.9.0',
	stuck: false
};

describe('ChartRolloutStatus', () => {
	it('shows installed and latest when they match', async () => {
		render(ChartRolloutStatus, { rollout: base });
		await expect.element(page.getByText(/Chart installed/)).toBeVisible();
		expect(document.body.textContent).not.toMatch(/upgrade pending|failing/);
	});

	it('says an upgrade is pending when behind', async () => {
		render(ChartRolloutStatus, { rollout: { ...base, latest: '0.10.0', desired: '0.10.0' } });
		await expect.element(page.getByText('(upgrade pending)')).toBeVisible();
		expect(document.querySelector('[role="status"]')).toBeNull();
	});

	it('warns when the upgrade is stuck', async () => {
		render(ChartRolloutStatus, {
			rollout: { installed: '0.9.0', desired: '0.10.0', latest: '0.10.0', stuck: true }
		});
		await expect.element(page.getByText('Chart upgrade failing')).toBeVisible();
		expect(document.body.textContent).not.toMatch(/upgrade pending/);
	});

	it('shows unknown for an unreadable installed version', async () => {
		render(ChartRolloutStatus, { rollout: { ...base, installed: null } });
		await expect.element(page.getByText('unknown', { exact: true })).toBeVisible();
	});

	it.each([null, undefined])('renders nothing without data (%s)', (rollout) => {
		render(ChartRolloutStatus, { rollout });
		expect(document.body.textContent?.trim()).toBe('');
	});
});
