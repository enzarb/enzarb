import type { Sql } from './db.ts';

/** One `work_item_changed` notification, resolved to the Project the item belongs to. */
export interface WorkItemChange {
	id: number;
	project: string | null;
}

type Listener = (change: WorkItemChange) => void;

/**
 * Fans one Postgres LISTEN connection on `work_item_changed` out to many SSE subscribers.
 * The LISTEN connection is opened with the first subscriber and closed with the last (the
 * connection must be direct, never through a transaction-mode pooler; postgres.js reconnects it
 * and re-issues LISTEN by itself).
 */
export class WorkEventHub {
	private listeners = new Set<Listener>();
	private stop: Promise<() => Promise<void>> | undefined;

	constructor(private sql: Sql) {}

	get size(): number {
		return this.listeners.size;
	}

	/** Returns an unsubscribe function. */
	subscribe(listener: Listener): () => void {
		this.listeners.add(listener);
		this.stop ??= this.open();
		return () => {
			this.listeners.delete(listener);
			if (this.listeners.size === 0) void this.close();
		};
	}

	private open(): Promise<() => Promise<void>> {
		const p = this.sql
			.listen('work_item_changed', (payload) => void this.dispatch(payload))
			.then((l) => () => l.unlisten());
		p.catch(() => {
			// Let the next subscribe() try again.
			if (this.stop === p) this.stop = undefined;
		});
		return p;
	}

	private async close(): Promise<void> {
		const stop = this.stop;
		this.stop = undefined;
		try {
			await (
				await stop
			)?.();
		} catch {
			/* connection already gone */
		}
	}

	private async dispatch(payload: string): Promise<void> {
		const id = Number(payload);
		if (!Number.isSafeInteger(id)) return;
		let rows: { project: string | null }[];
		try {
			rows = await this.sql<{ project: string | null }[]>`
				SELECT project FROM workflows_api.work_item_v WHERE id = ${id}`;
		} catch {
			return;
		}
		const project = rows[0]?.project ?? null;
		for (const l of [...this.listeners]) l({ id, project });
	}
}
