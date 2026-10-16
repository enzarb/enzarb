import { getContext, setContext } from 'svelte';
import { applyRow, type TaskRow } from '#lib/tasks.ts';

export interface NeedsYouDeps {
	list: () => Promise<TaskRow[]>;
	/** The changed row, or null when it is gone. */
	get: (id: number) => Promise<TaskRow | null>;
	watch: (onChange: (id: number | null) => void) => () => void;
}

/**
 * The live "Waiting on you" queue (projection rows with `needs_you`) behind the nav badge and the
 * Tasks page. One watcher serves the whole app; `onChange` fans each change out so pages need no
 * socket of their own. Membership is the engine's `needs_you`, never decided here.
 */
export class NeedsYouStore {
	items = $state.raw<TaskRow[]>([]);
	loaded = $state(false);
	failed = $state(false);
	readonly count = $derived(this.items.length);

	// plain callback registry, nothing renders from it
	// eslint-disable-next-line svelte/prefer-svelte-reactivity
	#listeners = new Set<(id: number | null) => void>();
	#deps: NeedsYouDeps;

	constructor(deps: NeedsYouDeps) {
		this.#deps = deps;
	}

	start(): () => void {
		void this.refresh();
		return this.#deps.watch((id) => {
			if (id === null) void this.refresh();
			else void this.apply(id);
			for (const l of this.#listeners) l(id);
		});
	}

	onChange(fn: (id: number | null) => void): () => void {
		this.#listeners.add(fn);
		return () => this.#listeners.delete(fn);
	}

	async refresh(): Promise<void> {
		try {
			this.items = await this.#deps.list();
			this.failed = false;
		} catch {
			this.failed = true;
		} finally {
			this.loaded = true;
		}
	}

	async apply(id: number): Promise<void> {
		try {
			this.items = applyRow(this.items, id, await this.#deps.get(id), (t) => t.needsYou);
		} catch {
			await this.refresh();
		}
	}
}

const KEY = Symbol('enzarb.needs-you');

export const setNeedsYou = (store: NeedsYouStore) => setContext(KEY, store);
export const getNeedsYou = () => getContext<NeedsYouStore>(KEY);
