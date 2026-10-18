import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { FindingRow } from '#lib/tasks.ts';
import FindingList from './FindingList.svelte';

const NOW = Date.parse('2026-10-18T12:00:00Z');
const finding = (over: Partial<FindingRow> = {}): FindingRow => ({
	key: 'k1',
	taskId: 7,
	project: 'web',
	checkId: 'dead_wait',
	kind: 'dead_wait',
	severity: 'warning',
	text: 'Waiting on a PR that is gone',
	firstSeen: '2026-10-18T10:00:00Z',
	lastSeen: '2026-10-18T11:59:00Z',
	issueUrl: null,
	...over
});

describe('FindingList', () => {
	it('shows the healthy empty state', async () => {
		render(FindingList, { findings: [], now: NOW });
		await expect.element(page.getByText('The platform looks healthy.')).toBeVisible();
	});

	it('lists a task finding with age and a link to the task', async () => {
		render(FindingList, { findings: [finding()], now: NOW });
		await expect.element(page.getByText('Waiting on a PR that is gone')).toBeVisible();
		await expect.element(page.getByText('2h ago')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'Task #7' }))
			.toHaveAttribute('href', '/task/7');
		await expect.element(page.getByRole('link', { name: 'Filed issue' })).not.toBeInTheDocument();
	});

	it('shows project and check for a platform finding, and the issue link when set', async () => {
		render(FindingList, {
			findings: [
				finding({
					taskId: null,
					checkId: 'sandbox_down',
					kind: 'platform_failure',
					issueUrl: 'https://github.com/acme/web/issues/9'
				})
			],
			now: NOW
		});
		await expect.element(page.getByText('web · sandbox_down')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'Filed issue' }))
			.toHaveAttribute('href', 'https://github.com/acme/web/issues/9');
	});

	it('renders unknown severity and kind as given', async () => {
		render(FindingList, {
			findings: [finding({ severity: 'fatal', kind: 'novel_kind' })],
			now: NOW
		});
		await expect.element(page.getByText('fatal')).toBeVisible();
		await expect.element(page.getByText('novel_kind')).toBeVisible();
		expect(document.querySelector('.ez-finding--unknown')).not.toBeNull();
	});
});
