import { describe, expect, it, vi } from 'vitest';
import { NeedsYouStore, type NeedsYouDeps } from './needs-you.svelte.ts';
import type { WorkItemSummary } from './work-items.ts';

const item = (id: number, needs: boolean): WorkItemSummary => ({
	id,
	key: `github:a/b#${id}`,
	project: 'p',
	workflow: 'issue',
	state: needs ? 'AwaitingApproval' : 'Planning',
	title: `t${id}`,
	url: null,
	updatedAt: `2026-01-0${id}T00:00:00Z`,
	needs: needs ? { category: 'approval', reason: null } : null
});

function setup(overrides: Partial<NeedsYouDeps> = {}) {
	let emit: (id: number | null) => void = () => {};
	const deps: NeedsYouDeps = {
		list: vi.fn(async () => [item(1, true)]),
		get: vi.fn(async (id: number) => item(id, true)),
		submit: vi.fn(async () => {}),
		watch: (fn) => {
			emit = fn;
			return () => {};
		},
		...overrides
	};
	const store = new NeedsYouStore(deps);
	store.start();
	return { store, deps, emit: (id: number | null) => emit(id) };
}

describe('NeedsYouStore', () => {
	it('loads the queue and counts it', async () => {
		const { store } = setup();
		await vi.waitFor(() => expect(store.count).toBe(1));
		expect(store.loaded).toBe(true);
	});

	it('increments on a live change into a needs-you state and decrements when it leaves', async () => {
		const { store, deps, emit } = setup();
		await vi.waitFor(() => expect(store.count).toBe(1));
		emit(2);
		await vi.waitFor(() => expect(store.count).toBe(2));
		vi.mocked(deps.get).mockResolvedValueOnce(item(2, false));
		emit(2);
		await vi.waitFor(() => expect(store.count).toBe(1));
		expect(store.items.map((i) => i.id)).toEqual([1]);
	});

	it('refetches the whole list on null and falls back to it when a get fails', async () => {
		const { store, deps, emit } = setup();
		await vi.waitFor(() => expect(deps.list).toHaveBeenCalledTimes(1));
		emit(null);
		await vi.waitFor(() => expect(deps.list).toHaveBeenCalledTimes(2));
		vi.mocked(deps.get).mockRejectedValueOnce(new Error('boom'));
		emit(5);
		await vi.waitFor(() => expect(deps.list).toHaveBeenCalledTimes(3));
		expect(store.count).toBe(1);
	});

	it('fans changes out to page listeners until they unsubscribe', async () => {
		const { store, emit } = setup();
		const seen: (number | null)[] = [];
		const off = store.onChange((id) => seen.push(id));
		emit(3);
		off();
		emit(4);
		expect(seen).toEqual([3]);
	});

	it('removes a row optimistically on act and restores it when the action fails', async () => {
		const { store, deps } = setup();
		await vi.waitFor(() => expect(store.count).toBe(1));
		await store.act(1, 'approve');
		expect(deps.submit).toHaveBeenCalledWith(1, 'approve');
		expect(store.count).toBe(0);

		vi.mocked(deps.submit).mockRejectedValueOnce(new Error('409'));
		await store.refresh();
		expect(store.count).toBe(1);
		await expect(store.act(1, 'approve')).rejects.toThrow('409');
		await vi.waitFor(() => expect(store.count).toBe(1));
	});
});
