import postgres from 'postgres';
import { pushPayload, summaryPayload } from '#lib/push.ts';
import { classifyNeedsYou } from '#lib/work-items.ts';
import { getConfig } from './config.ts';
import { getSql, type Sql } from './db.ts';
import { getProjectStore } from './kube.ts';
import { deliver, getSend, type Send } from './push.ts';
import { pushStore, type PushStore } from './push-store.ts';

/** A work item as the notifier needs it: `work_item_v` plus the id of the transition into its state. */
export interface NotifyItem {
	id: number;
	project: string | null;
	workflow: string;
	state: string;
	stateData: unknown;
	updatedAt: string;
	/** Latest transition into `state`; null when the view has none (fallback: `updatedAt`). */
	transitionId: number | null;
	title: string;
}

export interface NotifierDeps {
	loadItem(id: number): Promise<NotifyItem | null>;
	/** The Project's owner (GitHub login), or null for unknown projects. */
	ownerOf(project: string): Promise<string | null>;
	store: Pick<PushStore, 'targets' | 'claim' | 'remove' | 'touch'>;
	send: Send;
}

export interface BurstOptions {
	/** More than this many notifications to one user inside `windowSecs` collapse into a summary. */
	limit: number;
	windowSecs: number;
}

export const DEFAULT_BURST: BurstOptions = { limit: 3, windowSecs: 60 };

/**
 * One notification per transition: the claim key is the state plus the transition into it, so a
 * later re-entry into the same state notifies again and a non-state update does not.
 */
export const stateKey = (i: Pick<NotifyItem, 'state' | 'transitionId' | 'updatedAt'>): string =>
	`${i.state}:${i.transitionId ?? i.updatedAt}`;

/**
 * Handles one changed work item: classify with the shared "needs you" classifier, skip Projects
 * nobody owns, skip owners without a device for the category, claim the transition in the sent-log
 * (only the claimant sends, which keeps two pods from double-sending) and deliver. Returns whether a
 * claim was won and a notification attempted. Run failures reach this as the `Failed` state.
 */
export function createItemHandler(deps: NotifierDeps, burst: BurstOptions = DEFAULT_BURST) {
	return async function handle(id: number): Promise<boolean> {
		const item = await deps.loadItem(id);
		if (!item?.project) return false;
		const needs = classifyNeedsYou(item);
		if (!needs) return false;
		const owner = await deps.ownerOf(item.project);
		if (!owner) return false;
		const targets = await deps.store.targets(owner, needs.category);
		if (targets.length === 0) return false;
		const claim = await deps.store.claim(
			owner,
			item.id,
			needs.category,
			stateKey(item),
			burst.windowSecs
		);
		if (!claim.claimed) return false;
		const payload =
			claim.recent > burst.limit ? summaryPayload(claim.recent) : pushPayload(item, needs.category);
		await deliver(deps.store, deps.send, owner, targets, payload);
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

function loadItemFrom(sql: Sql) {
	return async (id: number): Promise<NotifyItem | null> => {
		const [r] = await sql<
			{
				id: string | number;
				key: string;
				project: string | null;
				workflow: string;
				state: string;
				state_data: unknown;
				updated_at: Date;
				transition_id: string | number | null;
			}[]
		>`
			SELECT w.id, w.key, w.project, w.workflow, w.state, w.state_data, w.updated_at,
				(SELECT max(t.id) FROM workflows_api.transition_v t
					WHERE t.work_item_id = w.id AND t.to_state = w.state) AS transition_id
			FROM workflows_api.work_item_v w WHERE w.id = ${id}`;
		if (!r) return null;
		const sd = r.state_data as { title?: unknown } | null;
		return {
			id: Number(r.id),
			project: r.project,
			workflow: r.workflow,
			state: r.state,
			stateData: r.state_data,
			updatedAt: r.updated_at.toISOString(),
			transitionId: r.transition_id == null ? null : Number(r.transition_id),
			title: typeof sd?.title === 'string' && sd.title.trim() ? sd.title : r.key
		};
	};
}

const PRUNE_EVERY_MS = 6 * 60 * 60 * 1000;
const KEEP_DAYS = 30;

let started: { stop(): Promise<void> } | undefined;

/**
 * Starts the listener in this process (idempotent; a no-op without VAPID keys). Every UI pod
 * listens; the sent-log claim makes sure each transition is sent by exactly one of them. The
 * `postgres` driver reconnects and re-LISTENs by itself.
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
				console.warn(`notifier: work item ${id}: ${e instanceof Error ? e.message : 'failed'}`)
			);
		}
	});
	// A dedicated connection: LISTEN must not sit in the pool.
	const listener = postgres(getConfig().databaseUrl, { max: 1, onnotice: () => {} });
	void listener
		.listen('work_item_changed', (payload) => {
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
