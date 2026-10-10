import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import CreatedProjects from './CreatedProjects.svelte';
import type { ProjectView } from '#lib/project-types.ts';

const view = (name: string, conditions: ProjectView['conditions'], ready = false): ProjectView => ({
	name,
	displayName: name,
	namespace: null,
	repositories: [],
	triggers: { issues: true, changeRequests: true, mentions: true },
	conditions,
	ready
});
const created = (name: string) => ({
	name,
	status: 'created' as const,
	repositories: ['github:a/b']
});
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

describe('CreatedProjects', () => {
	it('lists failures and shows a checklist per created project from one poll', async () => {
		const load = vi.fn(async () => [
			view('one', [{ type: 'TenantReady', status: 'True' }]),
			view('two', [])
		]);
		render(CreatedProjects, {
			results: [
				created('one'),
				created('two'),
				{ name: 'bad', status: 'failed', repositories: [], reason: 'nope' }
			],
			load,
			intervalMs: 20
		});
		await expect.element(page.getByText('2 created, 1 failed.')).toBeInTheDocument();
		await expect.element(page.getByText('nope')).toBeInTheDocument();
		await expect
			.poll(() => page.getByRole('list', { name: 'Boot progress' }).elements().length)
			.toBe(2);
	});

	it('stops polling once every project is Ready', async () => {
		let ready = false;
		const load = vi.fn(async () => [
			view('one', [{ type: 'Ready', status: ready ? 'True' : 'False' }], ready)
		]);
		render(CreatedProjects, { results: [created('one')], load, intervalMs: 20 });
		await expect.poll(() => load.mock.calls.length).toBeGreaterThan(1);
		ready = true;
		await sleep(80);
		const calls = load.mock.calls.length;
		await sleep(150);
		expect(load.mock.calls.length).toBe(calls);
	});

	it('stops polling when every project failed terminally', async () => {
		const load = vi.fn(async () => [
			view('one', [{ type: 'TenantReady', status: 'False', reason: 'TenantFailed' }]),
			view('two', [{ type: 'Ready', status: 'False', reason: 'InvalidSpec' }])
		]);
		render(CreatedProjects, { results: [created('one'), created('two')], load, intervalMs: 20 });
		await sleep(120);
		expect(load.mock.calls.length).toBe(1);
	});

	it('keeps polling while one project is still booting', async () => {
		const load = vi.fn(async () => [
			view('one', [{ type: 'Ready', status: 'True' }], true),
			view('two', [{ type: 'TenantReady', status: 'True' }])
		]);
		render(CreatedProjects, { results: [created('one'), created('two')], load, intervalMs: 20 });
		await expect.poll(() => load.mock.calls.length).toBeGreaterThan(3);
	});
});
