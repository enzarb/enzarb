import type { Sql } from './db.ts';

/** pg_type oid of `text`, for `sql.array`. */
const TEXT_OID = 25;

/** A device as shown in Settings: never the keys. */
export interface PushDevice {
	endpoint: string;
	categories: string[];
	userAgent: string | null;
	createdAt: string;
}

/** What the sender needs to encrypt and post to one device. */
export interface PushTarget {
	endpoint: string;
	p256dh: string;
	auth: string;
}

export interface NewSubscription {
	endpoint: string;
	p256dh: string;
	auth: string;
	categories: string[];
	userAgent: string | null;
}

export interface Claim {
	/** True only for the first caller of this (owner, item, category, state_key). */
	claimed: boolean;
	/** Claims by this owner within the burst window, this one included. */
	recent: number;
}

/** The entry into a "needs you" category the owner was notified for (`app.push_item`). */
export interface PushEntry {
	category: string;
	stateKey: string;
	/** A notification tagged `item-<id>` was shown (not only a burst summary). */
	shown: boolean;
}

/**
 * Storage for push subscriptions and the sent-log, through the `app_api.push_*` functions only.
 * Every method takes the owner explicitly; callers pass the authenticated session login (remote
 * functions) or the project owner (the notifier), never client input.
 */
export function pushStore(sql: Sql) {
	return {
		/** False when the endpoint already belongs to another owner (nothing is changed). */
		async upsert(owner: string, s: NewSubscription): Promise<boolean> {
			const [row] = await sql<{ ok: boolean }[]>`
				SELECT app_api.push_subscription_upsert(
					${owner}, ${s.endpoint}, ${s.p256dh}, ${s.auth}, ${sql.array(s.categories, TEXT_OID)}, ${s.userAgent}) AS ok`;
			return row?.ok ?? false;
		},

		async list(owner: string): Promise<PushDevice[]> {
			const rows = await sql<
				{ endpoint: string; categories: string[]; user_agent: string | null; created_at: Date }[]
			>`SELECT endpoint, categories, user_agent, created_at FROM app_api.push_subscription_list(${owner})`;
			return rows.map((r) => ({
				endpoint: r.endpoint,
				categories: r.categories as string[],
				userAgent: r.user_agent,
				createdAt: r.created_at.toISOString()
			}));
		},

		/** The owner's devices subscribed to `category`; every device when it is null. */
		async targets(owner: string, category: string | null): Promise<PushTarget[]> {
			return sql<PushTarget[]>`
				SELECT endpoint, p256dh, auth FROM app_api.push_subscription_targets(${owner}, ${category})`;
		},

		async setCategories(owner: string, endpoint: string, categories: string[]): Promise<boolean> {
			const [row] = await sql<{ ok: boolean }[]>`
				SELECT app_api.push_subscription_set_categories(${owner}, ${endpoint}, ${sql.array(categories, TEXT_OID)}) AS ok`;
			return row?.ok ?? false;
		},

		/** Disable from Settings, or prune after the push service answered 404/410. */
		async remove(owner: string, endpoint: string): Promise<boolean> {
			const [row] = await sql<{ ok: boolean }[]>`
				SELECT app_api.push_subscription_delete(${owner}, ${endpoint}) AS ok`;
			return row?.ok ?? false;
		},

		async touch(owner: string, endpoint: string): Promise<void> {
			await sql`SELECT app_api.push_subscription_touch(${owner}, ${endpoint})`;
		},

		async claim(
			owner: string,
			workItemId: number,
			category: string,
			stateKey: string,
			windowSecs: number
		): Promise<Claim> {
			const [row] = await sql<{ claimed: boolean; recent: number }[]>`
				SELECT claimed, recent FROM app_api.push_claim(${owner}, ${workItemId}, ${category}, ${stateKey}, ${windowSecs})`;
			return { claimed: row?.claimed ?? false, recent: row?.recent ?? 0 };
		},

		/** Gives a claim back after a delivery in which every send failed transiently. */
		async release(
			owner: string,
			workItemId: number,
			category: string,
			stateKey: string
		): Promise<boolean> {
			const [row] = await sql<{ ok: boolean }[]>`
				SELECT app_api.push_claim_release(${owner}, ${workItemId}, ${category}, ${stateKey}) AS ok`;
			return row?.ok ?? false;
		},

		/** The open "needs you" entry of a work item for this owner, if any. */
		async entry(owner: string, workItemId: number): Promise<PushEntry | null> {
			const [row] = await sql<{ category: string; state_key: string; shown: boolean }[]>`
				SELECT category, state_key, shown FROM app_api.push_item_get(${owner}, ${workItemId})`;
			return row
				? { category: row.category as string, stateKey: row.state_key, shown: row.shown }
				: null;
		},

		async setEntry(owner: string, workItemId: number, entry: PushEntry): Promise<void> {
			await sql`SELECT app_api.push_item_set(${owner}, ${workItemId}, ${entry.category}, ${entry.stateKey}, ${entry.shown})`;
		},

		/** Closes the open entry; true for the one caller that should send the clear push. */
		async clearEntry(owner: string, workItemId: number): Promise<boolean> {
			const [row] = await sql<{ ok: boolean }[]>`
				SELECT app_api.push_item_clear(${owner}, ${workItemId}) AS ok`;
			return row?.ok ?? false;
		},

		async prune(olderThanDays: number): Promise<number> {
			const [row] = await sql<{ n: number; m: number }[]>`
				SELECT app_api.push_sent_prune(${olderThanDays}) AS n,
					app_api.push_item_prune(${olderThanDays}) AS m`;
			return (row?.n ?? 0) + (row?.m ?? 0);
		}
	};
}

export type PushStore = ReturnType<typeof pushStore>;
