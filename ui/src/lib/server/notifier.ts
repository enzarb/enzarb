import postgres from 'postgres';
import { clearPayload, pushPayload, summaryPayload } from '#lib/push.ts';
import { getConfig } from './config.ts';
import { getSql, type Sql } from './db.ts';
import { getProjectStore } from './kube.ts';
import { deliver, getSend, type Send } from './push.ts';
import { pushStore, type PushStore } from './push-store.ts';

/** A task as the notifier needs it: its `engine_api.task_v` projection row. */
export interface NotifyItem {
	id: number;
	project: string;
	title: string;
	needsYou: boolean;
	node: string;
	column: string | null;
	reason: string | null;
	/** Projection update time: part of the claim key of an entry into a wait. */
	updatedAt: string;
}

export interface NotifierDeps {
	loadItem(id: number): Promise<NotifyItem | null>;
	/** The Project's owner (GitHub login), or null for unknown projects. */
	ownerOf(project: string): Promise<string | null>;
	store: Pick<
		PushStore,
		'targets' | 'claim' | 'release' | 'remove' | 'touch' | 'entry' | 'setEntry' | 'clearEntry'
	>;
	send: Send;
}

export interface BurstOptions {
	/** More than this many notifications to one user inside `windowSecs` collapse into a summary. */
	limit: number;
	windowSecs: number;
}

export const DEFAULT_BURST: BurstOptions = { limit: 3, windowSecs: 60 };

/**
 * What the owner is waiting on, as the engine's projection says: the node with its declared column
 * and reason. Null when the projection does not have `needs_you`. The open entry (`app.push_item`)
 * stores this as its category, so updates that stay on the same wait never notify again, and
 * moving to another wait (or leaving and re-entering) does.
 */
export const entryCategory = (i: NotifyItem): string | null =>
	i.needsYou ? `${i.node}|${i.column ?? ''}|${i.reason ?? ''}` : null;

/**
 * Handles one changed projection row, skipping Projects nobody owns. If the owner's open entry is
 * for another wait, or the task no longer needs them, close it and (once, for a shown notification)
 * send a data-only clear push. Otherwise claim the entry in the sent-log (only the claimant sends,
 * which keeps two pods from double-sending), deliver to the owner's devices and record the entry.
 * Returns whether a claim was won and a notification attempted.
 */
export function createItemHandler(deps: NotifierDeps, burst: BurstOptions = DEFAULT_BURST) {
	return async function handle(id: number): Promise<boolean> {
		const item = await deps.loadItem(id);
		if (!item?.project) return false;
		const category = entryCategory(item);
		const owner = await deps.ownerOf(item.project);
		if (!owner) return false;
		const entry = await deps.store.entry(owner, item.id);
		if (entry && entry.category !== category) {
			if ((await deps.store.clearEntry(owner, item.id)) && entry.shown) {
				// Only for a shown notification: a clear with nothing to close may make the browser
				// show a generic notification (docs/notifications.md).
				const devices = await deps.store.targets(owner, null);
				await deliver(deps.store, deps.send, owner, devices, clearPayload(item.id));
			}
		}
		if (!category) return false;
		if (entry?.category === category) return false;
		const targets = await deps.store.targets(owner, null);
		if (targets.length === 0) return false;
		const key = `${category}:${item.updatedAt}`;
		const claim = await deps.store.claim(owner, item.id, category, key, burst.windowSecs);
		if (!claim.claimed) return false;
		const summary = claim.recent > burst.limit;
		const payload = summary ? summaryPayload(claim.recent) : pushPayload(item);
		let gone = 0;
		const counting: Send = async (t, p) => {
			const result = await deps.send(t, p);
			if (result === 'gone') gone++;
			return result;
		};
		const accepted = await deliver(deps.store, counting, owner, targets, payload);
		// Nothing accepted and no device reported gone: every send failed transiently, so release the
		// claim and let the next update to the task try again.
		if (accepted === 0 && gone === 0) {
			await deps.store.release(owner, item.id, category, key);
		} else if (accepted > 0) {
			await deps.store.setEntry(owner, item.id, { category, stateKey: key, shown: !summary });
		}
		return true;
	};
}

/**
 * Collects ids and flushes them once after `delayMs` of the first arrival, de-duplicated. Flushes
 * run one after another: ids that arrive during a flush go into the next one.
 */
export function createBatcher(
	flush: (ids: number[]) => Promise<void>,
	delayMs = 500,
	timers: { set: typeof setTimeout; clear: typeof clearTimeout } = {
		set: setTimeout,
		clear: clearTimeout
	}
) {
	const pending = new Set<number>();
	let timer: ReturnType<typeof setTimeout> | undefined;
	let chain: Promise<void> = Promise.resolve();

	function run() {
		timer = undefined;
		const ids = [...pending];
		pending.clear();
		chain = chain.then(() => flush(ids)).catch(() => {});
		return chain;
	}

	return {
		add(id: number) {
			pending.add(id);
			timer ??= timers.set(() => void run(), delayMs);
		},
		/** Resolves once everything flushed so far has been handled. */
		idle: () => chain,
		stop() {
			if (timer !== undefined) timers.clear(timer);
			timer = undefined;
			pending.clear();
		}
	};
}

const OWNER_TTL_MS = 15_000;

function createOwnerLookup(now: () => number = Date.now) {
	const cache = new Map<string, { at: number; owner: Promise<string | null> }>();
	return (project: string): Promise<string | null> => {
		const hit = cache.get(project);
		if (hit && now() - hit.at < OWNER_TTL_MS) return hit.owner;
		const owner = getProjectStore()
			.get(project)
			.then((p) => p?.spec.owner ?? null);
		cache.set(project, { at: now(), owner });
		owner.catch(() => {
			if (cache.get(project)?.owner === owner) cache.delete(project);
		});
		return owner;
	};
}

export function loadItemFrom(sql: Sql) {
	return async (id: number): Promise<NotifyItem | null> => {
		const [r] = await sql<
			{
				task_id: string | number;
				project: string;
				title: string;
				needs_you: boolean;
				node: string;
				column: string | null;
				reason: string | null;
				updated_at: Date;
			}[]
		>`
			SELECT task_id, project, title, needs_you, node, "column", reason, updated_at
			FROM engine_api.task_v WHERE task_id = ${id}`;
		if (!r) return null;
		return {
			id: Number(r.task_id),
			project: r.project,
			title: r.title,
			needsYou: r.needs_you,
			node: r.node,
			column: r.column,
			reason: r.reason,
			updatedAt: r.updated_at.toISOString()
		};
	};
}

const PRUNE_EVERY_MS = 6 * 60 * 60 * 1000;
const KEEP_DAYS = 30;

let started: { stop(): Promise<void> } | undefined;

/**
 * Starts the listener in this process (idempotent; a no-op without VAPID keys). Every UI pod
 * listens on `engine_projection`; the sent-log claim makes sure each entry is sent by exactly one
 * of them. The `postgres` driver reconnects and re-LISTENs by itself.
 */
export function startNotifier(): { stop(): Promise<void> } | null {
	if (started) return started;
	const send = getSend();
	if (!send) {
		console.info('notifier: no VAPID keys configured, push notifications are off');
		return null;
	}
	const sql = getSql();
	const store = pushStore(sql);
	const handle = createItemHandler({
		loadItem: loadItemFrom(sql),
		ownerOf: createOwnerLookup(),
		store,
		send
	});
	const batcher = createBatcher(async (ids) => {
		for (const id of ids) {
			await handle(id).catch((e) =>
				console.warn(`notifier: task ${id}: ${e instanceof Error ? e.message : 'failed'}`)
			);
		}
	});
	// A dedicated connection: LISTEN must not sit in the pool.
	const listener = postgres(getConfig().databaseUrl, { max: 1, onnotice: () => {} });
	void listener
		.listen('engine_projection', (payload) => {
			const id = Number(payload);
			if (Number.isSafeInteger(id) && id > 0) batcher.add(id);
		})
		.catch((e) => console.warn(`notifier: LISTEN failed: ${e instanceof Error ? e.message : e}`));
	const prune = () =>
		store.prune(KEEP_DAYS).catch((e) => console.warn(`notifier: prune failed: ${e?.message}`));
	void prune();
	const pruneTimer = setInterval(prune, PRUNE_EVERY_MS);
	pruneTimer.unref();
	started = {
		async stop() {
			clearInterval(pruneTimer);
			batcher.stop();
			await listener.end({ timeout: 5 });
			started = undefined;
		}
	};
	return started;
}
