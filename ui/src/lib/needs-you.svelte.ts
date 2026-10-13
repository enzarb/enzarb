import { getContext, setContext } from 'svelte';
import { applyNeedsYouChange, type WorkAction, type WorkItemSummary } from '#lib/work-items.ts';

export interface NeedsYouDeps {
	list: () => Promise<{ items: WorkItemSummary[]; total: number }>;
	get: (id: number) => Promise<WorkItemSummary>;
	submit: (id: number, action: WorkAction) => Promise<unknown>;
	watch: (onChange: (id: number | null) => void) => () => void;
}

/**
 * The live "needs you" queue behind both the nav badge and the `/work` list. One watcher serves the
 * whole app; `onChange` fans each change out so pages need no socket of their own.
 */
export class NeedsYouStore {
	items = $state<WorkItemSummary[]>([]);
	loaded = $state(false);
	failed = $state(false);
	/** Items waiting beyond those loaded (the queue is capped server-side), fixed at the last full load. */
	overflow = $state(0);
	readonly count = $derived(this.items.length + this.overflow);

	// plain callback registry, nothing renders from it
	// eslint-disable-next-line svelte/prefer-svelte-reactivity
	#listeners = new Set<(id: number | null) => void>();
	#deps: NeedsYouDeps;

	constructor(deps: NeedsYouDeps) {
		this.#deps = deps;
	}

	/** Starts the live subscription and the first load; returns the stop function. */
	start(): () => void {
		void this.refresh();
		return this.#deps.watch((id) => {
			if (id === null) void this.refresh();
			else void this.apply(id);
			for (const l of this.#listeners) l(id);
		});
	}

	/** Receive every live change (`null` = refresh everything). Returns the unsubscribe function. */
	onChange(fn: (id: number | null) => void): () => void {
		this.#listeners.add(fn);
		return () => this.#listeners.delete(fn);
	}

	async refresh(): Promise<void> {
		try {
			const { items, total } = await this.#deps.list();
			this.items = items;
			this.overflow = Math.max(0, total - items.length);
			this.failed = false;
		} catch {
			this.failed = true;
		} finally {
			this.loaded = true;
		}
	}

	/** Re-reads one changed item and adds, replaces or removes its row; falls back to a full refresh. */
	async apply(id: number): Promise<void> {
		try {
			this.items = applyNeedsYouChange(this.items, await this.#deps.get(id));
		} catch {
			await this.refresh();
		}
	}

	/** Submits an action; the row leaves optimistically and the list is reloaded if it fails. */
	async act(id: number, action: WorkAction): Promise<void> {
		const before = this.items;
		this.items = before.filter((w) => w.id !== id);
		try {
			await this.#deps.submit(id, action);
		} catch (e) {
			this.items = before;
			void this.refresh();
			throw e;
		}
	}
}

const KEY = Symbol('enzarb.needs-you');

export const setNeedsYou = (store: NeedsYouStore) => setContext(KEY, store);
export const getNeedsYou = () => getContext<NeedsYouStore>(KEY);
