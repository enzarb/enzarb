import { error } from '@sveltejs/kit';
import { command, getRequestEvent, query, requested } from '$app/server';
import * as v from 'valibot';
import type { TaskList } from '#lib/tasks.ts';
import { IMPLEMENT_WORKFLOW } from '#lib/tasks.ts';
import { getProjectStore } from '#lib/server/kube.ts';
import { projectService } from '#lib/server/project-service.ts';
import { invalidateToken, token } from '#lib/server/github-app.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import { getRepositoryAccess } from '#lib/server/repo-access.ts';
import {
	projectTasks,
	retryRun,
	startWorkflow,
	type ActionResult
} from '#lib/server/tasks-service.ts';
import { createGithubReads, type GithubReads } from '#lib/server/tasks-github.ts';
import { getTracks, TracksError } from '#lib/server/tracks.ts';

/**
 * Remote functions of the Tasks pages (Project → Tasks and the top-level Tasks): everything that
 * waits on a human. Owner only. Actions refresh the queries on the server in the same round trip
 * (`requested(...)` below, `.updates(...)` on the client), so the lists change without a reload.
 */

const LIMIT = 8;
const IdArg = v.pipe(v.string(), v.uuid());
const RepoName = v.pipe(v.string(), v.regex(/^[\w.-]+\/[\w.-]+$/), v.maxLength(200));
const Number_ = v.pipe(v.number(), v.integer(), v.minValue(1));

let reads: GithubReads | undefined;
const github = () => (reads ??= createGithubReads({ token, invalidate: invalidateToken }));

function deps() {
	return { tracks: getTracks(), github: github() };
}

function actionDeps() {
	const tracks = getTracks();
	if (!tracks) error(503, 'Tasks need enzarb-tracks, which is not enabled on this installation.');
	const access = getRepositoryAccess();
	return { tracks, checkAccess: (login: string, key: string) => access.check(login, key) };
}

async function allTasks(): Promise<TaskList> {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	const projects = await projectService(getProjectStore(), user.login).list();
	const lists = await Promise.all(projects.map((p) => projectTasks(deps(), p)));
	return {
		tasks: lists.flatMap((l) => l.tasks).sort((a, b) => b.at.localeCompare(a.at)),
		notes: [...new Set(lists.flatMap((l) => l.notes))]
	};
}

/** One Project's tasks. */
export const listProjectTasks = query(
	v.object({ name: NameArg }),
	async ({ name }): Promise<TaskList> => {
		const { project } = await ownedProject(name);
		return projectTasks(deps(), project);
	}
);

/** The tasks of all the signed-in user's Projects. */
export const listAllTasks = query(async (): Promise<TaskList> => allTasks());

/** The count in the main navigation. */
export const taskCount = query(async (): Promise<number> => {
	try {
		return (await allTasks()).tasks.length;
	} catch {
		return 0;
	}
});

/** Server-side refresh of every Tasks query the page holds, in the action's round trip. */
async function refreshAll(name: string) {
	// Async iteration: the query schemas validate asynchronously, which sync iteration rejects.
	for await (const e of requested(listProjectTasks, LIMIT)) {
		if (e.arg.name === name) void e.query.refresh();
		else e.ignore();
	}
	for await (const e of requested(listAllTasks, LIMIT)) void e.query.refresh();
	for await (const e of requested(taskCount, LIMIT)) void e.query.refresh();
}

/** The Implement button: start the implement workflow on a triaged issue. */
export const implementIssue = command(
	v.object({ name: NameArg, repo: RepoName, number: Number_ }),
	async ({ name, repo, number }): Promise<ActionResult> => {
		const { user, project } = await ownedProject(name);
		const r = await startWorkflow(actionDeps(), user.login, project, {
			repo,
			number,
			workflow: IMPLEMENT_WORKFLOW
		});
		await refreshAll(name);
		return r;
	}
);

/** Retry a failed run: a new run of the same workflow on the same subject. */
export const retryTaskRun = command(
	v.object({ name: NameArg, id: IdArg }),
	async ({ name, id }): Promise<ActionResult> => {
		const { user, project } = await ownedProject(name);
		const r = await retryRun(actionDeps(), user.login, project, id);
		await refreshAll(name);
		return r;
	}
);

/** Stop a run parked at a wait. */
export const stopTaskRun = command(
	v.object({ name: NameArg, id: IdArg }),
	async ({ name, id }): Promise<ActionResult> => {
		await ownedProject(name);
		const { tracks } = actionDeps();
		try {
			const run = await tracks.run(id);
			if (!run || run.project !== name) error(404, 'Run not found');
			await tracks.stopRun(id);
		} catch (e) {
			if (e instanceof TracksError)
				return { ok: false, message: 'enzarb-tracks could not be reached.' };
			throw e;
		}
		await refreshAll(name);
		return { ok: true, message: 'Stopping the run.', runId: id };
	}
);
