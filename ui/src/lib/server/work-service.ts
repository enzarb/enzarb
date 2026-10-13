import {
	availableActions,
	classifyNeedsYou,
	itemTitle,
	itemUrl,
	parseReadiness,
	type JobView,
	type TransitionView,
	type WorkAction,
	type WorkItemDetail,
	type WorkItemSummary
} from '#lib/work-items.ts';
import type { Sql } from './db.ts';
import { renderMarkdown } from './markdown.ts';

/** The signed-in GitHub user, in the engine's `Actor` shape (`id` is the GitHub id). */
export interface Actor {
	login: string;
	id: number;
}

export class WorkError extends Error {
	constructor(
		readonly kind: 'not-found' | 'invalid-action',
		message: string
	) {
		super(message);
	}
}

interface ItemRow {
	id: string | number;
	key: string;
	project: string | null;
	workflow: string;
	state: string;
	state_data: unknown;
	updated_at: Date;
}

/** Hard cap on the needs-you queue. */
const NEEDS_YOU_CAP = 200;

const iso = (d: Date | string) => new Date(d).toISOString();

function summary(r: ItemRow): WorkItemSummary {
	return {
		id: Number(r.id),
		key: r.key,
		project: r.project ?? '',
		workflow: r.workflow,
		state: r.state,
		title: itemTitle(r.key, r.state_data),
		url: itemUrl(r.key, r.state_data),
		updatedAt: iso(r.updated_at),
		needs: classifyNeedsYou({ workflow: r.workflow, state: r.state, stateData: r.state_data })
	};
}

/**
 * Markdown for a job's `output` (contracts §12). Agent jobs end `summary` with a fenced json
 * block `{decision, body}`; `body` is shown when present, otherwise the summary text itself.
 */
export function jobOutputMarkdown(output: unknown): string | null {
	if (output == null) return null;
	if (typeof output === 'string') return output.trim() || null;
	if (typeof output !== 'object') return null;
	const o = output as Record<string, unknown>;
	if (typeof o.summary === 'string' && o.summary.trim()) {
		const m = /```json\s*\n([\s\S]*?)\n\s*```\s*$/.exec(o.summary);
		if (m) {
			try {
				const block = JSON.parse(m[1]) as { decision?: unknown; body?: unknown };
				const head = o.summary.slice(0, m.index).trim();
				const decision =
					typeof block.decision === 'string' ? `**Decision:** ${block.decision}\n\n` : '';
				const body = typeof block.body === 'string' ? block.body : head;
				return (decision + body).trim() || null;
			} catch {
				/* not a decision block: show the text as written */
			}
		}
		return o.summary;
	}
	if (typeof o.error === 'string' && o.error.trim()) return `**Error:** ${o.error}`;
	return null;
}

/**
 * Work item reads and actions for one user. Reads go through the `workflows_api` views only;
 * `projects` are the names of the Projects the user owns (the views carry the project name).
 */
export function workService(sql: Sql, projects: readonly string[]) {
	const owned = new Set(projects);

	async function load(id: number): Promise<ItemRow | null> {
		if (owned.size === 0) return null;
		const [row] = await sql<ItemRow[]>`
			SELECT id, key, project, workflow, state, state_data, updated_at
			FROM workflows_api.work_item_v
			WHERE id = ${id} AND project IN ${sql([...owned])}`;
		return row ?? null;
	}

	return {
		async list(project?: string): Promise<WorkItemSummary[]> {
			const names = project === undefined ? [...owned] : owned.has(project) ? [project] : [];
			if (names.length === 0) return [];
			const rows = await sql<ItemRow[]>`
				SELECT id, key, project, workflow, state, state_data, updated_at
				FROM workflows_api.work_item_v
				WHERE project IN ${sql(names)}
				ORDER BY updated_at DESC, id DESC
				LIMIT 500`;
			return rows.map(summary);
		},

		/**
		 * Items waiting on the user, oldest first. Not limited by recency (only the hard cap), so old
		 * waiting items are never pushed out by newer activity.
		 */
		async listNeedsYou(): Promise<WorkItemSummary[]> {
			if (owned.size === 0) return [];
			const rows = await sql<ItemRow[]>`
				SELECT id, key, project, workflow, state, state_data, updated_at
				FROM workflows_api.work_item_v
				WHERE project IN ${sql([...owned])}
					AND (state IN ('AwaitingApproval', 'ReadyToMerge', 'AwaitingChanges', 'Failed', 'Stale')
						OR (workflow = 'change_request'
							AND state NOT IN ('Done', 'Duplicate', 'Cancelled', 'Merged', 'Closed')))
				ORDER BY updated_at ASC, id ASC`;
			return rows
				.map(summary)
				.filter((i) => i.needs)
				.slice(0, NEEDS_YOU_CAP);
		},

		/** Sanitized HTML of the latest plan job's output; null when there is none or the item is not owned. */
		async plan(id: number): Promise<string | null> {
			if (!(await load(id))) return null;
			const jobs = await sql<{ output: unknown }[]>`
				SELECT output FROM workflows_api.job_v
				WHERE work_item_id = ${id} AND kind = 'agent_plan' AND output IS NOT NULL
				ORDER BY created_at DESC, id DESC`;
			for (const j of jobs) {
				const md = jobOutputMarkdown(j.output);
				if (md) return renderMarkdown(md);
			}
			return null;
		},

		/** Null for missing items and items of projects the user does not own. */
		async get(id: number): Promise<WorkItemDetail | null> {
			const row = await load(id);
			if (!row) return null;
			const transitions = await sql<
				{
					id: string | number;
					from_state: string | null;
					to_state: string;
					event_type: string;
					at: Date;
				}[]
			>`
				SELECT id, from_state, to_state, event_type, at
				FROM workflows_api.transition_v WHERE work_item_id = ${id} ORDER BY at, id`;
			const jobs = await sql<
				{
					id: string | number;
					kind: string;
					status: string;
					attempt: number;
					updated_at: Date;
					output: unknown;
				}[]
			>`
				SELECT id, kind, status, attempt, updated_at, output
				FROM workflows_api.job_v WHERE work_item_id = ${id} ORDER BY created_at, id`;
			return {
				...summary(row),
				readiness: row.workflow === 'change_request' ? parseReadiness(row.state_data) : null,
				actions: availableActions(row.state),
				transitions: transitions.map((t): TransitionView => ({
					id: Number(t.id),
					fromState: t.from_state,
					toState: t.to_state,
					eventType: t.event_type,
					at: iso(t.at)
				})),
				jobs: jobs.map((j): JobView => {
					const md = jobOutputMarkdown(j.output);
					return {
						id: Number(j.id),
						kind: j.kind,
						status: j.status,
						attempt: j.attempt,
						updatedAt: iso(j.updated_at),
						outputHtml: md ? renderMarkdown(md) : null
					};
				})
			};
		},

		/** Validates against the item's current state, then calls `workflows_api.submit_action`. */
		async submitAction(id: number, actor: Actor, action: WorkAction): Promise<number | null> {
			const row = await load(id);
			if (!row) throw new WorkError('not-found', 'Work item not found');
			if (!availableActions(row.state).includes(action)) {
				throw new WorkError('invalid-action', `Cannot ${action} while ${row.state}`);
			}
			const [res] = await sql<{ id: string | number | null }[]>`
				SELECT workflows_api.submit_action(
					${id}::bigint, ${sql.json({ login: actor.login, id: actor.id, is_bot: false })}, ${action}, ${sql.json({})}) AS id`;
			return res?.id == null ? null : Number(res.id);
		}
	};
}
