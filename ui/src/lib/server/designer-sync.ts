import { requested, type RemoteQueryFunction } from '$app/server';

/** Most cache keys of one query a client may ask a single command to update. */
const LIMIT = 8;

type DesignerQuery<T> = RemoteQueryFunction<
	{ name: string; repo?: string | undefined },
	T,
	{ name: string; repo?: string | undefined }
>;

/**
 * Single-flight update of a designer query after a commit. The client names the query instances
 * it shows (`command().updates(query)`); each one is bound to the client's own cache key, so a page
 * that passes no `repo` (the default repository) is updated as well as one that passes the key.
 * Instances for the committed repository (`key`; `defaultKey` stands for an absent `repo`) are `set`
 * from the committed content (`value`, never read back from GitHub, whose contents API can lag a
 * PUT), or re-fetched when there is no `value`; every other instance is left alone.
 */
export async function updateRequested<T>(
	query: DesignerQuery<T>,
	name: string,
	key: string,
	defaultKey: string | undefined,
	value: T | undefined
) {
	// Async iteration: the query schemas validate asynchronously, which sync iteration rejects.
	for await (const e of requested(query, LIMIT)) {
		const repo = (e.arg.repo ?? defaultKey)?.toLowerCase();
		if (e.arg.name !== name || repo !== key) {
			e.ignore();
		} else if (value) {
			e.query.set(value);
		} else {
			void e.query.refresh();
		}
	}
}
