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

const client: TracksClient = {
	runs: async ({ project, eventIds }) =>
		project === E2E_PROJECT && (!eventIds || eventIds.includes(run.eventId)) ? [run] : [],
	run: async (id) => (id === E2E_RUN_ID ? structuredClone(run) : null),
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
	dryRun: async () => ({ results: [] }) as never,
	decisions: async () => []
};

export const getTracks = (): TracksClient => client;
