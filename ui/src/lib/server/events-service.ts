import { repoKeyFromUrl, summarizeEvent, type EventRow } from '#lib/events.ts';
import type { ProjectView } from '#lib/project-types.ts';
import type { Sql } from './db.ts';

export const EVENTS_PAGE_SIZE = 100;

interface Row {
	id: string | number;
	event: string;
	action: string | null;
	repo: string;
	sender: string | null;
	received_at: Date | string;
	payload: unknown;
}

/** Lowercase `owner/repo` keys of a project's repositories. */
export function projectRepoKeys(project: Pick<ProjectView, 'repositories'>): string[] {
	return project.repositories
		.map((r) => repoKeyFromUrl(r.url))
		.filter((k): k is string => k !== null);
}

/**
 * Read side of `connectors.event_v`: the webhook deliveries the receiver logged. Callers pass only
 * repositories of projects the user owns.
 */
export function eventsService(sql: Sql) {
	return {
		/** Newest first; `before` is the id of the last row of the previous page. */
		async list(repos: string[], before?: number): Promise<EventRow[]> {
			if (repos.length === 0) return [];
			const rows = await sql<Row[]>`
				SELECT id, event, action, repo, sender, received_at, payload
				FROM connectors.event_v
				WHERE repo = ANY(${sql.array(repos, 25)})
				  AND (${before ?? null}::bigint IS NULL OR id < ${before ?? null}::bigint)
				ORDER BY id DESC
				LIMIT ${EVENTS_PAGE_SIZE}`;
			return rows.map((r) => ({
				id: Number(r.id),
				event: r.event,
				action: r.action,
				repo: r.repo,
				sender: r.sender,
				summary: summarizeEvent(r.payload),
				receivedAt: new Date(r.received_at).toISOString()
			}));
		},
		/** The subset of `ids` that are events of `repos`. */
		async idsIn(repos: string[], ids: number[]): Promise<number[]> {
			if (repos.length === 0 || ids.length === 0) return [];
			const rows = await sql<{ id: string | number }[]>`
				SELECT id FROM connectors.event_v
				WHERE id = ANY(${sql.array(ids, 20)}) AND repo = ANY(${sql.array(repos, 25)})`;
			return rows.map((r) => Number(r.id));
		},
		/** The raw payload of one event, only when it belongs to one of `repos`. */
		async payload(repos: string[], id: number): Promise<unknown | null> {
			if (repos.length === 0) return null;
			const rows = await sql<{ payload: unknown }[]>`
				SELECT payload FROM connectors.event_v
				WHERE id = ${id} AND repo = ANY(${sql.array(repos, 25)})`;
			return rows[0]?.payload ?? null;
		},
		/** Events by id with their payloads (workflow dry runs), only those that belong to `repos`. */
		async withPayloads(
			repos: string[],
			ids: number[]
		): Promise<{ id: number; event: string; action: string | null; payload: unknown }[]> {
			if (repos.length === 0 || ids.length === 0) return [];
			const rows = await sql<Pick<Row, 'id' | 'event' | 'action' | 'payload'>[]>`
				SELECT id, event, action, payload FROM connectors.event_v
				WHERE id = ANY(${sql.array(ids, 20)}) AND repo = ANY(${sql.array(repos, 25)})`;
			return rows.map((r) => ({
				id: Number(r.id),
				event: r.event,
				action: r.action,
				payload: r.payload
			}));
		}
	};
}
