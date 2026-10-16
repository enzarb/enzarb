import { describe, expect, it, vi } from 'vitest';
import type { PushPayload } from '#lib/push.ts';
import {
	createBatcher,
	createItemHandler,
	entryCategory,
	type NotifierDeps,
	type NotifyItem
} from './notifier.ts';
import type { PushEntry, PushTarget } from './push-store.ts';

const device = (n: number): PushTarget => ({
	endpoint: `https://push.example/${n}`,
	p256dh: 'k',
	auth: 'a'
});

const item = (over: Partial<NotifyItem> = {}): NotifyItem => ({
	id: 1,
	project: 'proj',
	title: 'Add retries',
	needsYou: true,
	node: 'plan/approve',
	column: 'review',
	reason: 'Plan awaiting approval',
	updatedAt: '2026-10-13T10:00:00.000Z',
	...over
});

/** In-memory stand-in for the app_api.push_* functions: same claim semantics as the SQL. */
function setup(
	opts: {
		items?: NotifyItem[];
		owners?: Record<string, string>;
		devices?: Record<string, PushTarget[]>;
	} = {}
) {
	const items = new Map((opts.items ?? [item()]).map((i) => [i.id, i]));
	const owners = opts.owners ?? { proj: 'octo' };
	const devices = opts.devices ?? { octo: [device(1)] };
	const claims = new Set<string>();
	const sent: { target: PushTarget; payload: PushPayload & { type?: 'clear' } }[] = [];
	const removed: string[] = [];
	const gone = new Set<string>();
	const failing = new Set<string>();
	const entries = new Map<string, PushEntry & { cleared: boolean }>();
	const deps: NotifierDeps = {
		loadItem: async (id) => items.get(id) ?? null,
		ownerOf: async (p) => owners[p] ?? null,
		store: {
			targets: async (owner) => devices[owner] ?? [],
			claim: async (owner, id, category, key) => {
				const k = [owner, id, category, key].join('|');
				const claimed = !claims.has(k);
				claims.add(k);
				const recent = [...claims].filter((c) => c.startsWith(`${owner}|`)).length;
				return { claimed, recent };
			},
			release: async (owner, id, category, key) =>
				claims.delete([owner, id, category, key].join('|')),
			remove: async (_o, endpoint) => {
				removed.push(endpoint);
				return true;
			},
			touch: async () => {},
			entry: async (owner, id) => {
				const e = entries.get(`${owner}|${id}`);
				return e && !e.cleared
					? { category: e.category, stateKey: e.stateKey, shown: e.shown }
					: null;
			},
			setEntry: async (owner, id, e) =>
				void entries.set(`${owner}|${id}`, { ...e, cleared: false }),
			clearEntry: async (owner, id) => {
				const e = entries.get(`${owner}|${id}`);
				if (!e || e.cleared) return false;
				e.cleared = true;
				return e.shown;
			}
		},
		send: async (target, payload) => {
			sent.push({ target, payload: payload as PushPayload });
			if (failing.has(target.endpoint)) return 'error';
			return gone.has(target.endpoint) ? 'gone' : 'ok';
		}
	};
	return { items, entries, deps, sent, removed, gone, failing, handle: createItemHandler(deps) };
}

describe('projection needs_you', () => {
	it('notifies with the declared reason as the title', async () => {
		const t = setup();
		expect(await t.handle(1)).toBe(true);
		expect(t.sent).toHaveLength(1);
		expect(t.sent[0].payload).toMatchObject({ title: 'Plan awaiting approval', tag: 'item-1' });
	});

	it('stays quiet for tasks that do not need the user', async () => {
		const t = setup({ items: [item({ needsYou: false })] });
		expect(await t.handle(1)).toBe(false);
		expect(t.sent).toHaveLength(0);
	});

	it('keys an entry by node, column and reason', () => {
		expect(entryCategory(item())).toBe('plan/approve|review|Plan awaiting approval');
		expect(entryCategory(item({ needsYou: false }))).toBeNull();
	});

	it('ignores unknown tasks', async () => {
		expect(await setup().handle(999)).toBe(false);
	});
});

describe('transient failures', () => {
	it('releases the claim when every send errors, so the next update retries', async () => {
		const t = setup();
		t.failing.add(device(1).endpoint);
		await t.handle(1);
		t.failing.clear();
		await t.handle(1);
		expect(t.sent).toHaveLength(2);
	});

	it('keeps the claim when a device accepted', async () => {
		const t = setup({ devices: { octo: [device(1), device(2)] } });
		t.failing.add(device(1).endpoint);
		await t.handle(1);
		await t.handle(1);
		expect(t.sent).toHaveLength(2);
	});

	it('keeps the claim when the only device was gone', async () => {
		const t = setup();
		t.gone.add(device(1).endpoint);
		await t.handle(1);
		await t.handle(1);
		expect(t.sent).toHaveLength(1);
	});
});

describe('ownership', () => {
	it('sends nothing for a project nobody owns', async () => {
		const t = setup({ owners: {} });
		expect(await t.handle(1)).toBe(false);
		expect(t.sent).toHaveLength(0);
	});

	it('only sends to the devices of the project owner', async () => {
		const t = setup({ devices: { octo: [device(1)], mallory: [device(2)] } });
		await t.handle(1);
		expect(t.sent.map((s) => s.target.endpoint)).toEqual([device(1).endpoint]);
	});

	it('does not claim when the owner has no device for the category', async () => {
		const t = setup({ devices: {} });
		expect(await t.handle(1)).toBe(false);
		// enabling a device later still notifies for the same transition
		(t.deps.store as { targets: unknown }).targets = async () => [device(1)];
		expect(await t.handle(1)).toBe(true);
	});

	it('tasks without a project are ignored', async () => {
		const t = setup({ items: [item({ project: '' })] });
		expect(await t.handle(1)).toBe(false);
	});
});

describe('de-duplication', () => {
	it('sends once per transition even when two pods handle the same change', async () => {
		const t = setup();
		const [a, b] = await Promise.all([t.handle(1), t.handle(1)]);
		expect([a, b].filter(Boolean)).toHaveLength(1);
		expect(t.sent).toHaveLength(1);
	});

	it('does not repeat for a projection update on the same wait', async () => {
		const t = setup();
		await t.handle(1);
		t.items.set(
			1,
			item({ updatedAt: '2026-10-13T11:00:00.000Z', text: 'x' } as Partial<NotifyItem>)
		);
		expect(await t.handle(1)).toBe(false);
		expect(t.sent).toHaveLength(1);
	});

	it('sends again after leaving and re-entering the same wait', async () => {
		const t = setup();
		await t.handle(1);
		t.items.set(
			1,
			item({ needsYou: false, node: 'implement/run', updatedAt: '2026-10-13T11:00:00.000Z' })
		);
		await t.handle(1);
		t.items.set(1, item({ updatedAt: '2026-10-13T12:00:00.000Z' }));
		expect(await t.handle(1)).toBe(true);
		expect(t.sent.filter((s) => !('type' in s.payload))).toHaveLength(2);
	});
});

describe('wait entries', () => {
	it('clears once when the task stops needing the user', async () => {
		const t = setup();
		await t.handle(1);
		t.items.set(1, item({ needsYou: false }));
		expect(await t.handle(1)).toBe(false);
		await t.handle(1);
		expect(t.sent.map((s) => s.payload)).toEqual([
			expect.objectContaining({ title: 'Plan awaiting approval', tag: 'item-1' }),
			{ type: 'clear', tag: 'item-1' }
		]);
	});

	it('clears and notifies when the wait changes', async () => {
		const t = setup();
		await t.handle(1);
		t.items.set(1, item({ node: 'merge/ready', reason: 'Ready to merge' }));
		expect(await t.handle(1)).toBe(true);
		expect(t.sent.map((s) => ('type' in s.payload ? 'clear' : s.payload.title))).toEqual([
			'Plan awaiting approval',
			'clear',
			'Ready to merge'
		]);
	});

	it('sends no clear for a task that only got a burst summary', async () => {
		const items = [1, 2, 3, 4].map((id) => item({ id }));
		const t = setup({ items });
		for (const i of items) await t.handle(i.id);
		t.items.set(4, item({ id: 4, needsYou: false }));
		await t.handle(4);
		expect(t.sent).toHaveLength(4);
		expect(t.sent[3].payload.tag).toBe('work-summary');
	});

	it('links to the Tasks page with the card highlighted', async () => {
		const t = setup();
		await t.handle(1);
		expect(t.sent[0].payload).toMatchObject({ url: '/tasks?highlight=1' });
	});
});

describe('payload', () => {
	it('has only title, body, url and tag', async () => {
		const t = setup();
		await t.handle(1);
		expect(Object.keys(t.sent[0].payload).sort()).toEqual(['body', 'tag', 'title', 'url']);
	});
});

describe('bursts', () => {
	it('collapses notifications beyond the limit into one summary', async () => {
		const items = [1, 2, 3, 4, 5].map((id) => item({ id, title: `Item ${id}` }));
		const t = setup({ items });
		for (const i of items) await t.handle(i.id);
		const titles = t.sent.map((s) => s.payload.title);
		expect(titles.slice(0, 3)).toEqual(Array(3).fill('Plan awaiting approval'));
		expect(titles.slice(3)).toEqual(['4 tasks need you', '5 tasks need you']);
		expect(t.sent[4].payload.tag).toBe('work-summary');
		expect(t.sent[4].payload.url).toBe('/tasks');
	});
});

describe('gone subscriptions', () => {
	it('removes a device the push service reports as gone, and keeps the others', async () => {
		const t = setup({ devices: { octo: [device(1), device(2)] } });
		t.gone.add(device(1).endpoint);
		await t.handle(1);
		expect(t.removed).toEqual([device(1).endpoint]);
		expect(t.sent).toHaveLength(2);
	});
});

describe('createBatcher', () => {
	it('flushes de-duplicated ids once after the delay', async () => {
		vi.useFakeTimers();
		try {
			const flush = vi.fn(async () => {});
			const b = createBatcher(flush, 500);
			b.add(1);
			b.add(2);
			b.add(1);
			await vi.advanceTimersByTimeAsync(499);
			expect(flush).not.toHaveBeenCalled();
			await vi.advanceTimersByTimeAsync(2);
			expect(flush).toHaveBeenCalledExactlyOnceWith([1, 2]);
			b.add(3);
			await vi.advanceTimersByTimeAsync(500);
			expect(flush).toHaveBeenLastCalledWith([3]);
		} finally {
			vi.useRealTimers();
		}
	});

	it('keeps going after a failing flush', async () => {
		vi.useFakeTimers();
		try {
			const flush = vi.fn().mockRejectedValueOnce(new Error('boom')).mockResolvedValue(undefined);
			const b = createBatcher(flush, 10);
			b.add(1);
			await vi.advanceTimersByTimeAsync(10);
			b.add(2);
			await vi.advanceTimersByTimeAsync(10);
			await b.idle();
			expect(flush).toHaveBeenCalledTimes(2);
		} finally {
			vi.useRealTimers();
		}
	});
});
