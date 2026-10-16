import { describe, expect, it } from 'vitest';
import { row } from '#lib/test/task-row.ts';
import type { TaskRow } from '#lib/tasks.ts';
import { NeedsYouStore } from './needs-you.svelte.ts';

describe('NeedsYouStore', () => {
	it('follows the projection needs_you flag on live changes', async () => {
		let onChange: (id: number | null) => void = () => {};
		const rows = new Map<number, TaskRow>([[1, row({ id: 1, needsYou: true })]]);
		const store = new NeedsYouStore({
			list: async () => [...rows.values()].filter((r) => r.needsYou),
			get: async (id) => rows.get(id) ?? null,
			watch: (fn) => {
				onChange = fn;
				return () => {};
			}
		});
		const seen: (number | null)[] = [];
		store.onChange((id) => seen.push(id));
		store.start();
		await store.refresh();
		expect(store.count).toBe(1);

		rows.set(2, row({ id: 2, needsYou: true }));
		onChange(2);
		await new Promise((r) => setTimeout(r, 0));
		expect(store.items.map((r) => r.id).sort()).toEqual([1, 2]);

		rows.set(1, row({ id: 1, needsYou: false }));
		onChange(1);
		await new Promise((r) => setTimeout(r, 0));
		expect(store.items.map((r) => r.id)).toEqual([2]);
		expect(seen).toEqual([2, 1]);
	});
});
