import type { ProjectView } from '#lib/project-types.ts';
import { pendingPlan, type PendingPlan } from '#lib/plan.ts';
import {
	approvalBody,
	CHANGED_MESSAGE,
	foldStatus,
	mergeBlock,
	mergeTitle,
	summarizeChecks,
	type MergeOutcome,
	type MergeStep,
	type PullReview,
	type ReviewNotes
} from '#lib/pull-review.ts';
import type { RunDetail } from '#lib/runs.ts';
import { projectRepoKeys, type DesignerDeps } from './designer-files.ts';
import { renderMarkdown } from './markdown.ts';
import type { GithubReads } from './tasks-github.ts';
import type { GithubWrites } from './tasks-github-writes.ts';
import { allowed, type ActionResult } from './tasks-service.ts';
import { TracksError, type TracksClient } from './tracks.ts';

/**
 * Plan approval and pull request review inside Tasks, so nobody has to go to GitHub. Every write
 * re-checks the user's write access live first; tracks' resume endpoint and the App's tokens trust
 * this module for that.
 */

export const REVIEW_WORKFLOW = 'review-pull-requests';
const MAX_REVISION = 4000;

export interface ReviewDeps extends Pick<DesignerDeps, 'checkAccess'> {
	tracks: TracksClient;
	github: GithubReads;
	writes: GithubWrites;
}

type PlanDeps = Pick<ReviewDeps, 'tracks' | 'checkAccess'>;
type Project = Pick<ProjectView, 'name' | 'repositories'>;

const no = (message: string): ActionResult => ({ ok: false, message });
const unreachable = 'enzarb-tracks could not be reached.';

/** The plan a run waits on, rendered; null when the run is not waiting at a plan approval. */
export async function planOfRun(
	deps: Pick<ReviewDeps, 'tracks'>,
	projectName: string,
	id: string
): Promise<(PendingPlan & { html: string }) | null> {
	let run: RunDetail | null;
	try {
		run = await deps.tracks.run(id);
	} catch (e) {
		if (e instanceof TracksError) return null;
		throw e;
	}
	if (!run || run.project !== projectName) return null;
	const p = pendingPlan(run);
	return p ? { ...p, html: renderMarkdown(p.plan) } : null;
}

/** Approve or revise the plan a run waits on: access re-checked live, then tracks' resume. */
export async function answerPlan(
	deps: PlanDeps,
	login: string,
	project: Project,
	input: { id: string; action: 'approve' | 'revise'; text?: string }
): Promise<ActionResult> {
	const text = input.text?.trim() ?? '';
	if (input.action === 'revise' && !text) return no('Say what to change in the plan.');
	if (text.length > MAX_REVISION) return no(`Keep the revision under ${MAX_REVISION} characters.`);
	let run: RunDetail | null;
	try {
		run = await deps.tracks.run(input.id);
	} catch (e) {
		if (e instanceof TracksError) return no(unreachable);
		throw e;
	}
	if (!run || run.project !== project.name) return no('Run not found.');
	if (!pendingPlan(run)) return no('This run is not waiting at a plan approval.');
	const denied = await allowed(deps, login, project, run.repo);
	if (denied) return no(denied);
	try {
		const r = await deps.tracks.resumeRun({
			id: input.id,
			actor: login,
			action: input.action,
			...(input.action === 'revise' ? { text } : {})
		});
		if (r.result === 'not_waiting')
			return no('The run is no longer waiting, so nothing was sent. Reload to see where it is.');
		if (r.result === 'not_accepted')
			return no(
				`The run does not accept ${input.action === 'approve' ? 'an approval' : 'a revision'} right now${r.message ? `: ${r.message}` : '.'}`
			);
		return {
			ok: true,
			message:
				input.action === 'approve'
					? 'Plan approved: implementation started.'
					: 'Revision sent: the plan is being revised.',
			runId: input.id
		};
	} catch (e) {
		if (e instanceof TracksError) return no(unreachable);
		throw e;
	}
}

/** The newest review of this pull request: a run's summary, else the bot's marked comment. */
export async function reviewNotes(
	deps: Pick<ReviewDeps, 'tracks' | 'github'>,
	projectName: string,
	repo: string,
	n: number
): Promise<ReviewNotes | null> {
	try {
		const runs = (await deps.tracks.runs({ project: projectName }))
			.filter(
				(r) =>
					r.workflow === REVIEW_WORKFLOW &&
					r.repo.toLowerCase() === repo.toLowerCase() &&
					r.subject?.number === n &&
					!r.test &&
					r.status === 'done'
			)
			.sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
		for (const r of runs) {
			const d = await deps.tracks.run(r.id);
			const summary = [...(d?.nodeRuns ?? [])].reverse().flatMap((nr) => {
				const s = (nr.outputs as { result?: { summary?: unknown } } | null)?.result?.summary;
				return typeof s === 'string' && s.trim() ? [s] : [];
			})[0];
			if (summary)
				return {
					html: renderMarkdown(summary),
					run: { id: r.id, workflow: r.workflow },
					source: 'run'
				};
		}
	} catch (e) {
		if (!(e instanceof TracksError)) throw e;
	}
	const { items } = await deps.github.markedComments(repo, n);
	const last = items.at(-1);
	if (!last) return null;
	const body = last.replace(/<!--\s*ez-tracks:[^>]*-->/g, '').trim();
	return body ? { html: renderMarkdown(body), run: null, source: 'comment' } : null;
}

/** Everything the pull request view shows. Reads are cached by the GitHub reads. */
export async function pullReview(
	deps: Pick<ReviewDeps, 'tracks' | 'github'>,
	project: Project,
	repo: string,
	n: number
): Promise<PullReview> {
	const problems: string[] = [];
	const note = (s?: string) => {
		if (s && !problems.includes(s)) problems.push(s);
	};
	if (!projectRepoKeys(project).includes(`github:${repo.toLowerCase()}`))
		return {
			pull: null,
			files: [],
			filesTruncated: false,
			checks: [],
			notes: null,
			problems: ['That repository is not in this Project.']
		};
	const [{ pull, note: pn }, files, notes] = await Promise.all([
		deps.github.pull(repo, n),
		deps.github.pullFiles(repo, n),
		reviewNotes(deps, project.name, repo, n)
	]);
	note(pn);
	note(files.note);
	const checks = pull ? await deps.github.checkRuns(repo, pull.headSha) : { items: [] };
	note('note' in checks ? checks.note : undefined);
	return {
		pull,
		files: files.items,
		filesTruncated: files.truncated === true,
		checks: checks.items,
		notes,
		problems
	};
}

/**
 * Approve & merge, in this order, stopping at the first failure: (1) the user's write access,
 * live; (2) every cheap refusal on a fresh read, before anything is written: the head is still
 * the one the user reviewed (`sha`), not a draft, no conflicts, mergeability known, checks and
 * commit statuses green and fully read; (3) an approving review by the App whose body says who
 * approved (the App cannot act as the user; only when GitHub says the App cannot approve its own
 * pull request does the same sentence go in a comment instead); (4) a fresh re-read, then a squash
 * merge by the App titled `<title> (#n)`, guarded by the reviewed sha.
 */
export async function approveAndMerge(
	deps: ReviewDeps,
	login: string,
	project: Project,
	input: { repo: string; number: number; sha: string }
): Promise<MergeOutcome> {
	const steps: MergeStep[] = [
		{ id: 'access', label: 'Check your write access', result: 'skipped' },
		{ id: 'status', label: 'Check mergeability and checks', result: 'skipped' },
		{ id: 'approve', label: 'Approve the pull request', result: 'skipped' },
		{ id: 'merge', label: 'Squash and merge', result: 'skipped' }
	];
	const stop = (i: number, message: string): MergeOutcome => {
		steps[i]!.result = 'failed';
		steps[i]!.message = message;
		return { ok: false, message, steps };
	};
	const { repo, number: n, sha } = input;
	const unreadable = "Could not read the PR's current state, so nothing was merged.";

	const denied = await allowed(deps, login, project, repo);
	if (denied) return stop(0, denied);
	steps[0]!.result = 'done';

	const first = await deps.github.pull(repo, n, { fresh: true });
	if (first.note || !first.pull) return stop(1, unreadable);
	const pull = first.pull;
	if (pull.headSha !== sha) return stop(1, CHANGED_MESSAGE);
	const [runs, status] = await Promise.all([
		deps.github.checkRuns(repo, sha, { fresh: true }),
		deps.github.commitStatus(repo, sha, { fresh: true })
	]);
	if (runs.note || status.note || status.state === null)
		return stop(
			1,
			`Checks could not be read (${runs.note ?? status.note ?? 'unknown'}), so nothing was merged.`
		);
	if (runs.truncated) return stop(1, 'Too many checks to verify, so nothing was merged.');
	const block = mergeBlock(pull, foldStatus(summarizeChecks(runs.items), status.state));
	if (block) return stop(1, `Not merged. ${block}`);
	steps[1]!.result = 'done';

	const approved = await deps.writes.approve(repo, n, approvalBody(login), sha);
	if (approved.ok) steps[2]!.result = 'done';
	else if (approved.status === 422 && /own pull request/i.test(approved.message)) {
		const c = await deps.writes.comment(repo, n, approvalBody(login));
		if (!c.ok) return stop(2, `GitHub refused the approval: ${approved.message}`);
		steps[2]!.result = 'done';
		steps[2]!.message = 'Recorded as a comment: the App cannot approve its own pull request.';
	} else return stop(2, `GitHub refused the approval: ${approved.message}`);

	const again = await deps.github.pull(repo, n, { fresh: true });
	if (again.note || !again.pull) return stop(3, `Approved, not merged. ${unreadable}`);
	if (again.pull.headSha !== sha) return stop(3, `Approved, not merged. ${CHANGED_MESSAGE}`);
	const merged = await deps.writes.squashMerge(repo, n, {
		title: mergeTitle(again.pull.title, n),
		sha
	});
	if (!merged.ok) return stop(3, `Approved, not merged. GitHub refused: ${merged.message}`);
	steps[3]!.result = 'done';
	return { ok: true, message: `Approved and merged #${n}.`, steps };
}
