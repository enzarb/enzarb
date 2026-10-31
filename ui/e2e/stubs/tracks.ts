// Stand-in for src/lib/server/tracks.ts in the e2e web server: enzarb-tracks without the service.
// The "TOML" of both files is JSON here, and validation accepts everything, so the specs exercise
// the UI's own commit, refresh and render path against the real SvelteKit remote functions.
import type { ModesDoc } from '../../src/lib/modes-types.ts';
import type { WorkflowsFileWire } from '../../src/lib/workflows-types.ts';
import { E2E_PROJECT } from '../constants.ts';
import type { RunDetail } from '../../src/lib/runs.ts';
import { TracksError, type TracksClient } from '../../src/lib/server/tracks.ts';

export * from '../../src/lib/server/tracks.ts';

const parse = <T>(text: string): T => {
	try {
		return JSON.parse(text) as T;
	} catch {
		throw new TracksError(422, 'invalid_modes_toml', 'not valid');
	}
};

/** One seeded run: an agent node then a command node, still running until Stop. */
export const E2E_RUN_ID = '11111111-1111-4111-8111-111111111111';
const NOW = '2026-10-30T12:00:00.000Z';
const run: RunDetail = {
	id: E2E_RUN_ID,
	workflow: 'fix-issue',
	project: E2E_PROJECT,
	repo: 'e2e-org/app',
	subject: { kind: 'issue', number: 7 },
	eventId: 1,
	sha: 'abc1234def',
	status: 'running',
	test: false,
	currentNode: 'build',
	reason: null,
	stopRequested: false,
	createdAt: NOW,
	updatedAt: NOW,
	finishedAt: null,
	waitUntil: null,
	nodeRuns: [
		{
			node: 'triage',
			visit: 1,
			status: 'done',
			rendered: { type: 'agent', mode: 'triage', instruction: 'Triage issue #7', push: true },
			mode: 'triage',
			outputs: { result: { decision: 'go' }, exit: 'success' },
			usage: { input_tokens: 11, output_tokens: 22 },
			git: [
				{ repo: 'e2e-org/app', branch: 'ez/fix-issue/x', head_sha: 'deadbeefcafe', pushed: true }
			],
			error: null,
			intent: null,
			wait: null,
			startedAt: NOW,
			finishedAt: NOW
		},
		{
			node: 'say',
			visit: 1,
			status: 'done',
			rendered: { type: 'action', action: 'open_pull_request', title: 'Fix #7', body: 'Closes #7' },
			mode: null,
			outputs: { number: 42, url: 'https://github.com/e2e-org/app/pull/42' },
			usage: null,
			git: null,
			error: null,
			intent: null,
			wait: null,
			startedAt: NOW,
			finishedAt: NOW
		},
		{
			node: 'wait_reply',
			visit: 1,
			status: 'waiting',
			rendered: { type: 'wait', on: ['issue_comment.created'] },
			mode: null,
			outputs: null,
			usage: null,
			git: null,
			error: null,
			intent: null,
			wait: {
				on: ['issue_comment.created'],
				if: 'event.comment.user.login == subject.author',
				timeout_secs: 604800,
				until: '2026-11-06T12:00:00.000Z'
			},
			startedAt: NOW,
			finishedAt: null
		},
		{
			node: 'build',
			visit: 1,
			status: 'running',
			rendered: { type: 'command', task: 'build', args: ['go'] },
			mode: null,
			outputs: null,
			usage: null,
			git: null,
			error: null,
			intent: null,
			wait: null,
			startedAt: NOW,
			finishedAt: null
		}
	]
};

/** Runs the stub was asked to start, newest first (the Tasks e2e reads them through the page). */
export const started: { repo: string; number: number; workflow: string }[] = [];

export const E2E_WAIT_RUN_ID = '44444444-4444-4444-8444-444444444444';
export const E2E_FAILED_RUN_ID = '55555555-5555-4555-8555-555555555555';
const waitRun: RunDetail = {
	...run,
	id: E2E_WAIT_RUN_ID,
	workflow: 'ask-first',
	subject: { kind: 'issue', number: 11 },
	eventId: 2,
	status: 'waiting',
	currentNode: 'ask',
	waitUntil: '2026-11-06T12:00:00.000Z',
	nodeRuns: [
		{
			...run.nodeRuns[2]!,
			node: 'ask',
			wait: {
				on: ['issue_comment.created'],
				if: "event.comment.body == 'go'",
				timeout_secs: 604800,
				until: '2026-11-06T12:00:00.000Z'
			}
		}
	]
};
const failedRun: RunDetail = {
	...run,
	id: E2E_FAILED_RUN_ID,
	workflow: 'fix-ci',
	subject: { kind: 'issue', number: 12 },
	eventId: 3,
	status: 'failed',
	reason: 'timeout',
	currentNode: null,
	finishedAt: NOW,
	nodeRuns: []
};

/** Waiting at a plan approval: two runs, so one spec can approve and another revise. */
export const E2E_PLAN_RUN_ID = '66666666-6666-4666-8666-666666666666';
export const E2E_PLAN_REVISE_RUN_ID = '77777777-7777-4777-8777-777777777777';
const PLAN =
	'## Plan\n\n1. Guard the start-up path\n2. Add a **regression test**\n\n<script>alert(1)</script>';
const planRun = (id: string, number: number, eventId: number): RunDetail => ({
	...run,
	id,
	workflow: 'implement-labeled-issues',
	subject: { kind: 'issue', number },
	eventId,
	status: 'waiting',
	currentNode: 'approve_plan',
	waitUntil: '2026-11-13T12:00:00.000Z',
	nodeRuns: [
		{
			...run.nodeRuns[0]!,
			node: 'plan',
			outputs: { result: { plan: PLAN }, exit: 'success' }
		},
		{
			...run.nodeRuns[2]!,
			node: 'approve_plan',
			wait: {
				on: ['manual', 'issue_comment.created'],
				if: null,
				timeout_secs: 1209600,
				until: '2026-11-13T12:00:00.000Z'
			}
		}
	]
});
/** The newest review of PR #50: its summary is the cliff notes. */
const reviewRun: RunDetail = {
	...run,
	id: '88888888-8888-4888-8888-888888888888',
	workflow: 'review-pull-requests',
	subject: { kind: 'pull_request', number: 50 },
	eventId: 9,
	status: 'done',
	currentNode: null,
	finishedAt: NOW,
	nodeRuns: [
		{
			...run.nodeRuns[0]!,
			node: 'review',
			outputs: {
				result: { summary: '**Looks good.** The guard fixes the crash; one nit on naming.' }
			}
		}
	]
};
const more: RunDetail[] = [
	waitRun,
	failedRun,
	planRun(E2E_PLAN_RUN_ID, 13, 4),
	planRun(E2E_PLAN_REVISE_RUN_ID, 14, 5),
	reviewRun
];

/** What resume was asked, newest first. */
export const resumed: { id: string; actor: string; action: string; text?: string }[] = [];

const client: TracksClient = {
	runs: async ({ project, eventIds }) =>
		project === E2E_PROJECT
			? [run, ...more].filter((r) => !eventIds || eventIds.includes(r.eventId))
			: [],
	run: async (id) => {
		const r = [run, ...more].find((x) => x.id === id);
		return r ? structuredClone(r) : null;
	},
	resumeRun: async ({ id, actor, action, text }) => {
		const r = more.find((x) => x.id === id);
		if (!r || r.status !== 'waiting') return { result: 'not_waiting' };
		if (action !== 'approve' && action !== 'revise')
			return { result: 'not_accepted', message: 'only approve or revise' };
		resumed.unshift({ id, actor, action, text });
		// A revision re-plans and parks at the approval again; an approval moves on.
		if (action === 'approve') {
			r.status = 'running';
			r.currentNode = 'implement';
		}
		return { result: 'ok' };
	},
	stopRun: async (id) => {
		if (id !== E2E_RUN_ID || run.status !== 'running') return false;
		run.status = 'cancelled';
		run.reason = 'stopped';
		run.finishedAt = NOW;
		run.nodeRuns[run.nodeRuns.length - 1]!.status = 'failed';
		return true;
	},
	modesCatalog: async () => ({
		builtinTools: [],
		hookEvents: [],
		permissionValues: ['allow', 'ask', 'deny'],
		onFailureValues: ['block', 'warn'],
		templates: []
	}),
	parseModes: async (text) => ({ doc: parse<ModesDoc>(text), issues: [] }),
	validateModes: async () => [],
	renderModes: async (doc) => ({ text: JSON.stringify(doc), issues: [], rewritten: [] }),
	checkWorkflows: async (text) => ({ diagnostics: [], file: parse<WorkflowsFileWire>(text) }),
	renderWorkflows: async (file) => ({ text: JSON.stringify(file), diagnostics: [] }),
	startRun: async ({ repo, number, workflow }) => {
		started.unshift({ repo, number, workflow });
		more.unshift({
			...run,
			id: crypto.randomUUID(),
			workflow,
			repo,
			subject: { kind: 'issue', number },
			eventId: -started.length,
			status: 'running',
			nodeRuns: []
		});
		return { id: '33333333-3333-4333-8333-333333333333', alreadyRunning: false };
	},
	dryRun: async () => ({ results: [] }) as never,
	decisions: async () => []
};

export const getTracks = (): TracksClient => client;
