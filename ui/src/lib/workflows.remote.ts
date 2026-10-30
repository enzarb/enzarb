import { error } from '@sveltejs/kit';
import { command, query } from '$app/server';
import * as v from 'valibot';
import { RepoArg } from '#lib/modes-schema.ts';
import {
	DryRunSchema,
	RepoEventsSchema,
	SaveWorkflowsSchema,
	TEXT_MAX,
	WorkflowsFileSchema
} from '#lib/workflows-schema.ts';
import { getSql } from '#lib/server/db.ts';
import { eventsService } from '#lib/server/events-service.ts';
import { projectRepoKeys } from '#lib/server/designer-files.ts';
import { updateRequested } from '#lib/server/designer-sync.ts';
import { invalidateToken, token } from '#lib/server/github-app.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import { getRepositoryAccess } from '#lib/server/repo-access.ts';
import { getTracks } from '#lib/server/tracks.ts';
import {
	dryRunWorkflows as dryRun,
	loadWorkflows,
	saveWorkflows,
	tracksFailure,
	type WorkflowsDeps
} from '#lib/server/workflows-service.ts';

/** Remote functions of the workflow designer (Project → Workflows, ADR-0029 §6, §7). */

const UNAVAILABLE = {
	ok: false as const,
	code: 'unavailable' as const,
	message: 'The workflow designer needs enzarb-tracks, which is not enabled on this installation.'
};

function deps(): WorkflowsDeps | null {
	const tracks = getTracks();
	if (!tracks) return null;
	const access = getRepositoryAccess();
	return {
		tracks,
		checkAccess: (login, key) => access.check(login, key),
		token,
		invalidate: invalidateToken
	};
}

/** The `.ez/workflows.toml` of one of the Project's repositories, checked against its modes. */
export const getWorkflows = query(
	v.object({ name: NameArg, repo: RepoArg }),
	async ({ name, repo }) => {
		const { user, project } = await ownedProject(name);
		const d = deps();
		if (!d) return UNAVAILABLE;
		return loadWorkflows(d, user, project, repo);
	}
);

async function withTracks<T>(name: string, run: (d: WorkflowsDeps) => Promise<T>) {
	await ownedProject(name);
	const d = deps();
	if (!d) return UNAVAILABLE;
	try {
		return await run(d);
	} catch (e) {
		return tracksFailure(e);
	}
}

const ModesToml = v.nullable(v.pipe(v.string(), v.maxLength(TEXT_MAX)));

/** Raw TOML to the graph (switching tabs, and validating the TOML editor). */
export const checkWorkflowsText = command(
	v.object({
		name: NameArg,
		text: v.pipe(v.string(), v.maxLength(TEXT_MAX)),
		modesToml: ModesToml
	}),
	({ name, text, modesToml }) =>
		withTracks(name, async (d) => ({
			ok: true as const,
			...(await d.tracks.checkWorkflows(text, modesToml))
		}))
);

/** The graph to TOML with its report: validates the graph as it is edited. */
export const renderWorkflowsFile = command(
	v.object({ name: NameArg, file: WorkflowsFileSchema, modesToml: ModesToml }),
	({ name, file, modesToml }) =>
		withTracks(name, async (d) => ({
			ok: true as const,
			...(await d.tracks.renderWorkflows(file, modesToml))
		}))
);

/** Commit to the default branch: write access re-checked, never with errors, blob-sha concurrency. */
export const saveWorkflowsFile = command(SaveWorkflowsSchema, async ({ name, ...input }) => {
	const { user, project } = await ownedProject(name);
	const d = deps();
	if (!d) return UNAVAILABLE;
	if (!user.githubId) error(400, 'Your session has no GitHub id; sign in again');
	const r = await saveWorkflows(d, user, project, { ...input });
	if (!r.ok) return r;
	const { loaded, ...result } = r;
	// Single flight: the page's own query instances are set from the committed content and sha.
	if (!r.unchanged)
		await updateRequested(
			getWorkflows,
			name,
			input.repo.toLowerCase(),
			projectRepoKeys(project)[0],
			loaded
		);
	return result;
});

/** Events logged for one of the Project's repositories (the dry run's event picker). */
export const listRepoEvents = query(RepoEventsSchema, async ({ name, repo, before }) => {
	const { project } = await ownedProject(name);
	const key = repo.toLowerCase();
	if (!projectRepoKeys(project).includes(key)) return [];
	return eventsService(getSql()).list([key.replace(/^github:/, '')], before);
});

/** Dry-runs the (unsaved) workflows against logged events: no sandbox, GitHub or model calls. */
export const dryRunWorkflows = command(DryRunSchema, async ({ name, ...input }) => {
	const { user, project } = await ownedProject(name);
	const tracks = getTracks();
	if (!tracks) return UNAVAILABLE;
	const events = eventsService(getSql());
	const access = getRepositoryAccess();
	return dryRun(
		{
			tracks,
			checkAccess: (login, key) => access.check(login, key),
			events: (repo, ids) => events.withPayloads([repo], ids)
		},
		user.login,
		project,
		input
	);
});
