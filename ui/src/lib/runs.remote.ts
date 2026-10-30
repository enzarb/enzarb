import { error } from '@sveltejs/kit';
import { command, query, requested } from '$app/server';
import * as v from 'valibot';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import { getTracks, TracksError } from '#lib/server/tracks.ts';
import type { RunDetail, RunSummary } from '#lib/runs.ts';

/** Remote functions of the Runs tab (Project → Runs, ADR-0029 section 7). Owner only. */

/** Most cache keys of one query a single command may ask to update. */
const LIMIT = 8;

const IdArg = v.pipe(v.string(), v.uuid());

function tracks() {
	const t = getTracks();
	if (!t) error(503, 'Runs need enzarb-tracks, which is not enabled on this installation.');
	return t;
}

function upstream(e: unknown): never {
	if (e instanceof TracksError) error(502, 'enzarb-tracks could not be reached');
	throw e;
}

/** The Project's runs, newest first. */
export const listRuns = query(
	v.object({ name: NameArg }),
	async ({ name }): Promise<RunSummary[]> => {
		await ownedProject(name);
		const t = tracks();
		try {
			return await t.runs({ project: name });
		} catch (e) {
			return upstream(e);
		}
	}
);

/** The runs started by the given events (the Events tab links to them). */
export const listRunsForEvents = query(
	v.object({
		name: NameArg,
		ids: v.pipe(v.array(v.pipe(v.number(), v.integer(), v.minValue(1))), v.maxLength(200))
	}),
	async ({ name, ids }): Promise<RunSummary[]> => {
		await ownedProject(name);
		const t = getTracks();
		if (!t || ids.length === 0) return [];
		try {
			return (await t.runs({ project: name, eventIds: ids })).filter((r) => r.project === name);
		} catch (e) {
			if (e instanceof TracksError) return [];
			throw e;
		}
	}
);

/** One run with its node runs. 404 unless it belongs to this Project. */
export const getRun = query(
	v.object({ name: NameArg, id: IdArg }),
	async ({ name, id }): Promise<RunDetail> => {
		await ownedProject(name);
		const t = tracks();
		let run: RunDetail | null;
		try {
			run = await t.run(id);
		} catch (e) {
			return upstream(e);
		}
		if (!run || run.project !== name) error(404, 'Run not found');
		return run;
	}
);

/**
 * Stop a run (the owner only). The executor cancels it at its next check; the queries the page
 * shows are refreshed on the server in the same round trip (`.updates(...)` on the client).
 */
export const stopRun = command(v.object({ name: NameArg, id: IdArg }), async ({ name, id }) => {
	await ownedProject(name);
	const t = tracks();
	let current: RunDetail | null;
	let stopping: boolean;
	try {
		current = await t.run(id);
		if (!current || current.project !== name) error(404, 'Run not found');
		stopping = await t.stopRun(id);
	} catch (e) {
		return upstream(e);
	}
	// Async iteration: the query schemas validate asynchronously, which sync iteration rejects.
	for await (const e of requested(getRun, LIMIT)) {
		if (e.arg.name === name && e.arg.id === id) void e.query.refresh();
		else e.ignore();
	}
	for await (const e of requested(listRuns, LIMIT)) {
		if (e.arg.name === name) void e.query.refresh();
		else e.ignore();
	}
	return { stopping };
});
