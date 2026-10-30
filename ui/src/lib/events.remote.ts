import { command, getRequestEvent, query } from '$app/server';
import * as v from 'valibot';
import { getSql } from '#lib/server/db.ts';
import { eventsService, projectRepoKeys } from '#lib/server/events-service.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import { getTracks, TracksError } from '#lib/server/tracks.ts';
import type { EventDecision } from '#lib/events.ts';
import { issueRealtimeConnection } from '#lib/server/realtime-connection.ts';

/** Sets the HttpOnly realtime cookie for the session user; returns the stream URL, never a token. */
export const realtimeConnection = command(async () => issueRealtimeConnection(getRequestEvent()));

const ListArg = v.object({
	name: NameArg,
	before: v.optional(v.pipe(v.number(), v.integer(), v.minValue(1)))
});

/** Events logged for the repositories of a project, newest first. */
export const listEvents = query(ListArg, async ({ name, before }) => {
	const { project } = await ownedProject(name);
	return eventsService(getSql()).list(projectRepoKeys(project), before);
});

const PayloadArg = v.object({
	name: NameArg,
	id: v.pipe(v.number(), v.integer(), v.minValue(1))
});

/** The raw payload of one event of the project; `null` when it is not one of its events. */
export const getEventPayload = query(PayloadArg, async ({ name, id }) => {
	const { project } = await ownedProject(name);
	return (await eventsService(getSql()).payload(projectRepoKeys(project), id)) ?? null;
});

const DecisionsArg = v.object({
	name: NameArg,
	ids: v.pipe(v.array(v.pipe(v.number(), v.integer(), v.minValue(1))), v.maxLength(200))
});

/**
 * What enzarb-tracks decided about events of the project (ADR-0029): which workflow started, or
 * why none did. Empty when tracks is off or unreachable; the Events tab then shows no decisions.
 */
export const listDecisions = query(
	DecisionsArg,
	async ({ name, ids }): Promise<EventDecision[]> => {
		const { project } = await ownedProject(name);
		const tracks = getTracks();
		if (!tracks || ids.length === 0) return [];
		const own = await eventsService(getSql()).idsIn(projectRepoKeys(project), ids);
		if (own.length === 0) return [];
		try {
			return await tracks.decisions(own);
		} catch (e) {
			if (e instanceof TracksError) return [];
			throw e;
		}
	}
);
