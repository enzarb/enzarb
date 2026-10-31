import type { ProjectView } from '#lib/project-types.ts';
import type { RunDetail, RunSummary } from '#lib/runs.ts';
import { waitText } from '#lib/runs.ts';
import {
	IMPLEMENT_WORKFLOW,
	RUN_BRANCH_PREFIX,
	TRIAGE_LABEL,
	type Task,
	type TaskList
} from '#lib/tasks.ts';
import { fullName, projectRepoKeys, type DesignerDeps } from './designer-files.ts';
import type { GithubReads } from './tasks-github.ts';
import { TracksError, type TracksClient } from './tracks.ts';

/**
 * The Tasks page's server side: what waits on a human, per Project. Runs come from tracks, pull
 * requests and triaged issues from the GitHub App (cached, rate-limit aware: `tasks-github.ts`).
 * Actions are generic: Stop, Retry and Implement all go through tracks' `POST /v1/runs` / stop;
 * no workflow-specific logic lives here beyond the names in `tasks.ts`.
 */

export interface TasksDeps {
	tracks: TracksClient | null;
	github: GithubReads;
}

export interface ActionDeps extends Pick<DesignerDeps, 'checkAccess'> {
	tracks: TracksClient;
}

const subjectUrl = (r: RunSummary) =>
	r.subject ? (r.subject.url ?? `https://github.com/${r.repo}/issues/${r.subject.number}`) : null;

const runUrl = (project: string, id: string) =>
	`/projects/${encodeURIComponent(project)}/runs/${id}`;

const keyOf = (repo: string, n: number) => `${repo.toLowerCase()}#${n}`;

/** A live run counts for dedupe; finished ones only when they were the implement run. */
export async function projectTasks(
	deps: TasksDeps,
	project: Pick<ProjectView, 'name' | 'repositories'>
): Promise<TaskList> {
	const notes: string[] = [];
	const tasks: Task[] = [];
	let runs: RunSummary[] = [];
	if (deps.tracks) {
		try {
			runs = (await deps.tracks.runs({ project: project.name })).filter((r) => !r.test);
		} catch (e) {
			if (!(e instanceof TracksError)) throw e;
			notes.push('enzarb-tracks could not be reached, so runs are missing');
		}
	}

	// A run only waits on a human while its subject is open: one listing per repo, and an
	// unknown answer (GitHub unavailable) keeps everything.
	const openBy = new Map<string, Set<number> | null>();
	for (const repo of new Set(runs.filter((r) => r.subject).map((r) => r.repo.toLowerCase()))) {
		const { open, note } = await deps.github.openSubjects(repo);
		openBy.set(repo, open);
		if (note && !notes.includes(note)) notes.push(note);
	}
	const isClosed = (r: RunSummary) =>
		!!r.subject && openBy.get(r.repo.toLowerCase())?.has(r.subject.number) === false;

	// Parked runs: what they wait for needs the node runs.
	const waiting = runs.filter((r) => r.status === 'waiting' && !isClosed(r));
	const details = await Promise.all(
		waiting.map((r) => deps.tracks!.run(r.id).catch((): RunDetail | null => null))
	);
	waiting.forEach((r, i) => {
		const wait = [...(details[i]?.nodeRuns ?? [])]
			.reverse()
			.find((n) => n.status === 'waiting' && (n.wait || n.rendered?.type === 'wait'));
		tasks.push({
			kind: 'wait',
			key: `run:${r.id}`,
			project: project.name,
			repo: r.repo,
			number: r.subject?.number ?? null,
			title: `${r.workflow}${r.currentNode ? ` is waiting at ${r.currentNode}` : ''}`,
			githubUrl: subjectUrl(r),
			runUrl: runUrl(project.name, r.id),
			at: r.updatedAt,
			runId: r.id,
			workflow: r.workflow,
			waitingFor: wait ? waitText(wait) : '',
			until: r.waitUntil ?? wait?.wait?.until ?? null
		});
	});

	// A subject's latest run of a workflow decides whether a failure still needs you.
	const latest = new Map<string, RunSummary>();
	// Sorted here (stable, newest first) so the result does not depend on the API's order.
	const newestFirst = [...runs].sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
	for (const r of newestFirst) {
		const k = `${r.repo}#${r.subject?.number ?? ''}\0${r.workflow}`;
		if (!latest.has(k)) latest.set(k, r);
	}
	for (const r of latest.values()) {
		if (r.status !== 'failed' || isClosed(r)) continue;
		tasks.push({
			kind: 'failed',
			key: `run:${r.id}`,
			project: project.name,
			repo: r.repo,
			number: r.subject?.number ?? null,
			title: `${r.workflow} failed`,
			githubUrl: subjectUrl(r),
			runUrl: runUrl(project.name, r.id),
			at: r.finishedAt ?? r.updatedAt,
			runId: r.id,
			workflow: r.workflow,
			reason: r.reason
		});
	}

	// Implement runs, by subject, so triaged issues already started are left out.
	const started = new Set(
		runs
			.filter((r) => r.workflow === IMPLEMENT_WORKFLOW && r.subject)
			.map((r) => keyOf(r.repo, r.subject!.number))
	);
	for (const key of projectRepoKeys(project)) {
		const repo = fullName(key);
		const [pulls, issues] = await Promise.all([
			deps.github.pulls(repo),
			deps.github.issues(repo, TRIAGE_LABEL)
		]);
		for (const n of [pulls.note, issues.note]) if (n && !notes.includes(n)) notes.push(n);
		for (const p of pulls.items) {
			if (!p.branch.startsWith(RUN_BRANCH_PREFIX)) continue;
			tasks.push({
				kind: 'review',
				key: `pr:${repo}#${p.number}`,
				project: project.name,
				repo,
				number: p.number,
				title: p.title,
				githubUrl: p.url,
				// The run that opened it: branch `ez/<workflow>/<run id>`.
				runUrl: runLinkOfBranch(project.name, p.branch, runs),
				at: p.updatedAt,
				branch: p.branch,
				draft: p.draft
			});
		}
		for (const i of issues.items) {
			if (started.has(keyOf(repo, i.number))) continue;
			tasks.push({
				kind: 'triaged',
				key: `issue:${repo}#${i.number}`,
				project: project.name,
				repo,
				number: i.number,
				title: i.title,
				body: i.body,
				githubUrl: i.url,
				runUrl: null,
				at: i.updatedAt
			});
		}
	}
	tasks.sort((a, b) => b.at.localeCompare(a.at));
	return { tasks, notes };
}

function runLinkOfBranch(project: string, branch: string, runs: RunSummary[]): string | null {
	const id = branch.split('/')[2];
	return id && runs.some((r) => r.id === id) ? runUrl(project, id) : null;
}

export type ActionResult =
	{ ok: true; message: string; runId: string | null } | { ok: false; message: string };

const no = (message: string): ActionResult => ({ ok: false, message });

async function allowed(
	deps: ActionDeps,
	login: string,
	project: Pick<ProjectView, 'repositories'>,
	repo: string
): Promise<string | null> {
	const key = `github:${repo.toLowerCase()}`;
	if (!projectRepoKeys(project).includes(key)) return 'That repository is not in this Project.';
	if (!(await deps.checkAccess(login, key)))
		return 'You need write access to that repository (and the enzarb App installed on it).';
	return null;
}

/** Start `workflow` on `repo#number` by hand: write access re-checked live, then tracks. */
export async function startWorkflow(
	deps: ActionDeps,
	login: string,
	project: Pick<ProjectView, 'repositories'>,
	input: { repo: string; number: number; workflow: string }
): Promise<ActionResult> {
	const denied = await allowed(deps, login, project, input.repo);
	if (denied) return no(denied);
	try {
		const r = await deps.tracks.startRun({ ...input, actor: login });
		return r.alreadyRunning
			? { ok: true, message: `${input.workflow} is already running there.`, runId: null }
			: { ok: true, message: `Started ${input.workflow} on #${input.number}.`, runId: r.id };
	} catch (e) {
		if (e instanceof TracksError)
			return no(
				e.status >= 500 || e.code === 'unavailable'
					? 'enzarb-tracks could not be reached.'
					: e.message
			);
		throw e;
	}
}

/** Retry a failed run: a fresh run of the same workflow on the same subject. */
export async function retryRun(
	deps: ActionDeps,
	login: string,
	project: Pick<ProjectView, 'name' | 'repositories'>,
	id: string
): Promise<ActionResult> {
	let run: RunDetail | null;
	try {
		run = await deps.tracks.run(id);
	} catch (e) {
		if (e instanceof TracksError) return no('enzarb-tracks could not be reached.');
		throw e;
	}
	if (!run || run.project !== project.name) return no('Run not found.');
	if (run.status !== 'failed') return no('Only failed runs can be retried.');
	if (!run.subject) return no('This run has no issue or pull request to start again on.');
	return startWorkflow(deps, login, project, {
		repo: run.repo,
		number: run.subject.number,
		workflow: run.workflow
	});
}
