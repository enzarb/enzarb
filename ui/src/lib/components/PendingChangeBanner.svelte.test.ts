import { page } from 'vitest/browser';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import PendingChangeBanner from './PendingChangeBanner.svelte';

const change = { generation: 1, since: '2026-10-12T03:00:00Z', reason: 'maxRuns 2 -> 4' };

afterEach(() => vi.useRealTimers());

describe('PendingChangeBanner', () => {
	it('shows nothing without a pending change', async () => {
		render(PendingChangeBanner, { change: null, runningJobs: 0 });
		await expect.element(page.getByTestId('pending-change')).not.toBeInTheDocument();
	});

	it('waits for running jobs', async () => {
		render(PendingChangeBanner, { change, runningJobs: 2 });
		await expect.element(page.getByText('maxRuns 2 -> 4')).toBeVisible();
		const items = page.getByRole('listitem');
		await expect.element(items.nth(0)).toHaveAttribute('data-state', 'done');
		await expect.element(items.nth(1)).toHaveAttribute('data-state', 'active');
		await expect.element(items.nth(1)).toHaveTextContent('Waiting for running jobs to finish');
	});

	it('applies when no jobs run', async () => {
		render(PendingChangeBanner, { change, runningJobs: 0 });
		const items = page.getByRole('listitem');
		await expect.element(items.nth(1)).toHaveAttribute('data-state', 'done');
		await expect.element(items.nth(2)).toHaveAttribute('data-state', 'active');
	});

	it('shows the applied state when the change clears, then hides', async () => {
		vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
		const view = render(PendingChangeBanner, { change, runningJobs: 0 });
		await view.rerender({ change: null, runningJobs: 0 });
		const items = page.getByRole('listitem');
		await expect.element(items.nth(3)).toHaveAttribute('data-state', 'done');
		await vi.advanceTimersByTimeAsync(5001);
		await expect.element(page.getByTestId('pending-change')).not.toBeInTheDocument();
	});
});

describe('PendingChangeBanner roll now', () => {
	it('offers Roll now only while a change is pending and an action is given', async () => {
		const roll = vi.fn().mockResolvedValue(undefined);
		const view = render(PendingChangeBanner, { change, runningJobs: 1, roll });
		await expect.element(page.getByRole('button', { name: 'Roll now' })).toBeVisible();
		await view.rerender({ change: null, runningJobs: 0, roll });
		await expect.element(page.getByRole('button', { name: 'Roll now' })).not.toBeInTheDocument();
	});

	it('shows no button without the action', async () => {
		render(PendingChangeBanner, { change, runningJobs: 1 });
		await expect.element(page.getByRole('button', { name: 'Roll now' })).not.toBeInTheDocument();
	});
});
