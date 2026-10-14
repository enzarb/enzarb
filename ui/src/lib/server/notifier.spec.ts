import { describe, expect, it, vi } from 'vitest';
import type { PushPayload } from '#lib/push.ts';
import {
	createBatcher,
	createItemHandler,
	stateKey,
	type NotifierDeps,
	type NotifyItem
} from './notifier.ts';
import type { PushTarget } from './push-store.ts';

const device = (n: number): PushTarget => ({
	endpoint: `https://push.example/${n}`,
	p256dh: 'k',
	auth: 'a'
});

const item = (over: Partial<NotifyItem> = {}): NotifyItem => ({
	id: 1,
	project: 'proj',
	workflow: 'issue',
	state: 'AwaitingApproval',
	stateData: { title: 'Add retries', token: 'ghp_secret' },
	updatedAt: '2026-10-13T10:00:00.000Z',
	transitionId: 10,
	title: 'Add retries',
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
	const sent: { target: PushTarget; payload: PushPayload }[] = [];
	const removed: string[] = [];
	const gone = new Set<string>();
	const failing = new Set<string>();
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
			touch: async () => {}
		},
		send: async (target, payload) => {
			sent.push({ target, payload });
			if (failing.has(target.endpoint)) return 'error';
			return gone.has(target.endpoint) ? 'gone' : 'ok';
		}
	};
	return { items, deps, sent, removed, gone, failing, handle: createItemHandler(deps) };
}

describe('classification reuse', () => {
	const cases: [string, Partial<NotifyItem>, string][] = [
		['approval', { workflow: 'issue', state: 'AwaitingApproval' }, 'Plan awaiting your approval'],
		[
			'merge',
			{ workflow: 'change_request', state: 'ReadyToMerge' },
			'Change request ready to merge'
		],
		[
			'blocked (changes requested)',
			{ workflow: 'change_request', state: 'AwaitingChanges' },
			'Change request blocked'
		],
		[
			'blocked (failed check)',
			{
				workflow: 'change_request',
				state: 'Open',
				stateData: { checklist: { ci: { status: 'failed', detail: 'x' } } }
			},
			'Change request blocked'
		],
		['failed', { workflow: 'issue', state: 'Failed' }, 'Work item failed'],
		['stale', { workflow: 'change_request', state: 'Stale' }, 'Work item failed']
	];

	for (const [name, over, title] of cases) {
		it(`notifies for ${name}`, async () => {
			const t = setup({ items: [item(over)] });
			expect(await t.handle(1)).toBe(true);
			expect(t.sent).toHaveLength(1);
			expect(t.sent[0].payload.title).toBe(title);
			expect(t.sent[0].payload.url).toBe('/work/1');
		});
	}

	it('stays quiet for items that do not need the user', async () => {
		for (const over of [
			{ state: 'Planning' },
			{ state: 'Done' },
			{ workflow: 'change_request', state: 'Merged' }
		]) {
			const t = setup({ items: [item(over)] });
			expect(await t.handle(1)).toBe(false);
			expect(t.sent).toHaveLength(0);
		}
	});

	it('ignores unknown items', async () => {
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

	it('items without a project are ignored', async () => {
		const t = setup({ items: [item({ project: null })] });
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

	it('does not repeat for a non-state update of the same transition', async () => {
		const t = setup();
		await t.handle(1);
		t.items.set(1, item({ updatedAt: '2026-10-13T11:00:00.000Z' }));
		expect(await t.handle(1)).toBe(false);
		expect(t.sent).toHaveLength(1);
	});

	it('sends again for a new transition into the same state', async () => {
		const t = setup();
		await t.handle(1);
		t.items.set(1, item({ transitionId: 11 }));
		expect(await t.handle(1)).toBe(true);
		expect(t.sent).toHaveLength(2);
	});

	it('falls back to updated_at when the view has no transition', () => {
		expect(stateKey(item({ transitionId: null }))).toBe(
			'AwaitingApproval:2026-10-13T10:00:00.000Z'
		);
		expect(stateKey(item())).toBe('AwaitingApproval:10');
	});
});

describe('payload', () => {
	it('carries no state_data, token or reason', async () => {
		const t = setup({
			items: [
				item({
					state: 'Failed',
					stateData: { title: 'T', error: 'secret failure', token: 'ghp_secret' }
				})
			]
		});
		await t.handle(1);
		const json = JSON.stringify(t.sent[0].payload);
		expect(Object.keys(t.sent[0].payload).sort()).toEqual(['body', 'tag', 'title', 'url']);
		expect(json).not.toContain('ghp_secret');
		expect(json).not.toContain('secret failure');
	});
});

describe('bursts', () => {
	it('collapses notifications beyond the limit into one summary', async () => {
		const items = [1, 2, 3, 4, 5].map((id) => item({ id, title: `Item ${id}` }));
		const t = setup({ items });
		for (const i of items) await t.handle(i.id);
		const titles = t.sent.map((s) => s.payload.title);
		expect(titles.slice(0, 3)).toEqual(Array(3).fill('Plan awaiting your approval'));
		expect(titles.slice(3)).toEqual(['4 items need you', '5 items need you']);
		expect(t.sent[4].payload.tag).toBe('work-summary');
		expect(t.sent[4].payload.url).toBe('/work');
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
