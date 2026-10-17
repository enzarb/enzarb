import { page, userEvent } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { row } from '#lib/test/task-row.ts';
import type { SessionEvent } from '#lib/tasks.ts';
import ArtifactViewer from './ArtifactViewer.svelte';
import NeedsYouList from './NeedsYouList.svelte';
import SecretsForm from './SecretsForm.svelte';
import SessionInspector from './SessionInspector.svelte';
import StepRuns from './StepRuns.svelte';
import TaskBoard from './TaskBoard.svelte';
import TaskControls from './TaskControls.svelte';
import WorkflowViewer from './WorkflowViewer.svelte';

describe('TaskBoard', () => {
	it('renders the columns from the board config and places cards by column', async () => {
		render(TaskBoard, {
			board: {
				columns: [
					{ id: 'doing', title: 'In progress', limit: 1 },
					{ id: 'review', title: 'Review' }
				]
			},
			tasks: [
				row({ id: 1, title: 'A', column: 'doing' }),
				row({ id: 2, title: 'B', column: 'doing' }),
				row({ id: 3, title: 'C', column: 'review' })
			]
		});
		const cols = page.getByRole('region');
		await expect.element(cols.nth(0)).toHaveAccessibleName('In progress');
		await expect.element(cols.nth(0)).toHaveTextContent('2 / 1');
		await expect.element(cols.nth(0).getByRole('link', { name: 'A' })).toBeVisible();
		await expect.element(cols.nth(1)).toHaveAccessibleName('Review');
		await expect.element(cols.nth(1).getByRole('link', { name: 'C' })).toBeVisible();
	});
});

describe('Waiting on you', () => {
	it('groups cards by project with reason, text, artifacts, subjects and buttons', async () => {
		const oncommand = vi.fn(async () => {});
		render(NeedsYouList, {
			oncommand,
			tasks: [
				row({
					id: 7,
					project: 'web',
					title: 'Add search',
					needsYou: true,
					reason: 'Plan awaiting approval',
					text: 'Approve the plan or comment.',
					actions: [
						{ label: 'Approve', command: 'approve' },
						{ label: 'Replan', command: 'replan' }
					],
					artifacts: [{ title: 'Plan', format: 'markdown' }],
					subjects: [
						{ kind: 'issue', key: 'github:acme/web#1', url: 'https://github.com/acme/web/issues/1' }
					]
				}),
				row({ id: 8, project: 'api', title: 'Other', needsYou: true })
			]
		});
		await expect.element(page.getByRole('region', { name: 'Project api' })).toBeVisible();
		const web = page.getByRole('region', { name: 'Project web' });
		await expect.element(web).toHaveTextContent('Plan awaiting approval');
		await expect.element(web).toHaveTextContent('Approve the plan or comment.');
		await expect
			.element(web.getByRole('link', { name: 'Plan' }))
			.toHaveAttribute('href', '/task/7#artifact-0');
		await expect
			.element(web.getByRole('link', { name: 'acme/web#1' }))
			.toHaveAttribute('href', 'https://github.com/acme/web/issues/1');
		await web.getByRole('button', { name: 'Replan' }).click();
		expect(oncommand).toHaveBeenCalledWith(7, 'replan');
	});

	it('shows an empty state', async () => {
		render(NeedsYouList, { tasks: [], oncommand: async () => {} });
		await expect.element(page.getByText('Nothing is waiting on you')).toBeVisible();
	});
});

describe('TaskControls', () => {
	it('steers while running', async () => {
		const onsubmit = vi.fn(async () => {});
		render(TaskControls, { status: 'running', onsubmit });
		await page.getByLabelText('Steer the running agent').fill('use the other API');
		await page.getByRole('button', { name: 'Send steer' }).click();
		expect(onsubmit).toHaveBeenCalledWith('steer', 'use the other API');
		await page.getByRole('button', { name: 'Stop step' }).click();
		expect(onsubmit).toHaveBeenCalledWith('stop', undefined);
	});

	it('comments and resumes while paused', async () => {
		const onsubmit = vi.fn(async () => {});
		render(TaskControls, { status: 'paused', onsubmit });
		await page.getByLabelText('Comment').fill('/retry');
		await page.getByRole('button', { name: 'Comment' }).click();
		expect(onsubmit).toHaveBeenCalledWith('comment', '/retry');
		await page.getByRole('button', { name: 'Resume' }).click();
		expect(onsubmit).toHaveBeenCalledWith('resume', undefined);
	});
});

describe('SessionInspector', () => {
	const ev = (seq: number, kind: string, payload: unknown, truncated = false): SessionEvent => ({
		seq,
		at: '2026-10-16T12:00:00.000Z',
		kind,
		payload,
		truncated
	});
	const events = [
		ev(1, 'prompt', { text: 'Plan the change' }),
		ev(2, 'user', { text: 'user turn' }),
		ev(3, 'steer', { text: 'steer turn' }),
		ev(4, 'assistant', { text: 'assistant reply' }),
		ev(5, 'thinking', { text: 'deep thought' }),
		ev(6, 'tool_call', { name: 'Bash', input: { command: 'ls' } }),
		ev(7, 'tool_result', { name: 'Bash', output: 'boom', is_error: true }, true),
		ev(8, 'usage', { input_tokens: 1200, output_tokens: 300, cost_usd: 0.12 }),
		ev(9, 'system', { text: 'system note' }),
		ev(10, 'stdout', { text: 'out line' }),
		ev(11, 'stderr', { text: 'err line' })
	];

	it('renders every event kind, truncation and errors', async () => {
		const load = vi.fn(async (after: number) => (after === 0 ? events : []));
		render(SessionInspector, { load });
		await expect.element(page.getByText('11 events')).toBeVisible();
		for (const t of [
			'Plan the change',
			'user turn',
			'steer turn',
			'assistant reply',
			'system note',
			'out line',
			'err line'
		])
			await expect.element(page.getByText(t)).toBeInTheDocument();
		const usage = await vi.waitUntil(() => document.querySelector('[data-kind="usage"]'));
		expect(usage.textContent?.replace(/\s+/g, ' ')).toContain('1.2k in · 300 out · $0.12');
		await expect.element(page.getByText('truncated')).toBeVisible();
		// tool calls collapse
		await expect.element(page.getByText('"command": "ls"')).not.toBeVisible();
		await page.getByText('Bash').first().click();
		await expect.element(page.getByText('"command": "ls"')).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Jump to error' })).toBeVisible();
	});

	it('filters by search', async () => {
		render(SessionInspector, { load: async (after: number) => (after === 0 ? events : []) });
		await expect.element(page.getByText('11 events')).toBeVisible();
		await page.getByLabelText('Search this session').fill('assistant reply');
		await expect.element(page.getByText('1 of 11 events')).toBeVisible();
	});

	it('tails new events while live', async () => {
		let calls = 0;
		const load = vi.fn(async (after: number) => {
			calls++;
			return calls === 1 ? [events[0]] : after === 1 ? [events[3]] : [];
		});
		render(SessionInspector, { load, live: true, pollMs: 50 });
		await expect.element(page.getByText('assistant reply')).toBeVisible();
		await expect.element(page.getByText('live')).toBeVisible();
	});
});

describe('SessionInspector failures', () => {
	it('shows a notice when the session cannot be loaded', async () => {
		render(SessionInspector, {
			load: async () => {
				throw new Error('down');
			}
		});
		await expect.element(page.getByText('Could not load the session.')).toBeVisible();
	});

	it('pages on while a page is full', async () => {
		const full = Array.from({ length: 500 }, (_, i) => ({
			seq: i + 1,
			at: '2026-10-16T12:00:00.000Z',
			kind: 'system',
			payload: { text: `n${i + 1}` },
			truncated: false
		}));
		const load = vi.fn(async (after: number) =>
			after === 0 ? full : after === 500 ? [{ ...full[0], seq: 501 }] : []
		);
		render(SessionInspector, { load });
		await expect.element(page.getByText('501 events')).toBeVisible();
		expect(load).toHaveBeenCalledWith(500);
	});
});

describe('StepRuns', () => {
	const run = (runId: string, endedAt: string | null) => ({
		runId,
		node: 'n',
		step: `step-${runId}`,
		attempt: 1,
		runtime: null,
		model: null,
		startedAt: null,
		endedAt,
		activeSecs: null,
		queueSecs: null,
		exit: endedAt ? 'ok' : null,
		tokens: null,
		costUsd: null,
		resumes: 0,
		stalls: 0,
		steers: 0
	});

	it('opens a running step live and keeps ended steps collapsed', async () => {
		const load = vi.fn(async () => []);
		render(StepRuns, { runs: [run('a', '2026-10-16T12:00:00.000Z'), run('b', null)], load });
		await expect.element(page.getByText('live')).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Hide transcript' })).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Show transcript' })).toBeVisible();
		expect(load).toHaveBeenCalledWith('b', 0);
		expect(load).not.toHaveBeenCalledWith('a', 0);
	});

	it('shows an empty state for an ended step without events', async () => {
		render(StepRuns, { runs: [run('a', '2026-10-16T12:00:00.000Z')], load: async () => [] });
		await page.getByRole('button', { name: 'Show transcript' }).click();
		await expect.element(page.getByText('No session events captured.')).toBeVisible();
	});
});

describe('ArtifactViewer', () => {
	it('renders markdown (sanitized html), json and text', async () => {
		render(ArtifactViewer, {
			artifacts: [
				{ title: 'Plan', format: 'markdown', content: '## Plan', html: '<h2>Plan heading</h2>' },
				{ title: 'Data', format: 'json', content: '{"a":1}' },
				{ title: 'Log', format: 'text', content: 'plain text' }
			]
		});
		await page.getByText('Plan', { exact: true }).click();
		await expect.element(page.getByRole('heading', { name: 'Plan heading' })).toBeVisible();
		await page.getByText('Data', { exact: true }).click();
		await expect.element(page.getByText('"a": 1')).toBeVisible();
		await expect.element(page.getByText('plain text')).toBeVisible();
	});
});

describe('WorkflowViewer', () => {
	it('shows bindings, phases with params, exits, origin badges and lint', async () => {
		render(WorkflowViewer, {
			view: {
				bindings: [
					{ id: 'issues', on: 'issue', use: 'std/workflows/issue', origin: { layer: 'library' } }
				],
				workflows: [
					{
						ref: 'std/workflows/issue',
						phases: [
							{
								id: 'plan',
								use: 'std/phases/plan',
								origin: {
									layer: 'library',
									repo: 'enzarb/workflows',
									sha: 'abcdef1234',
									path: 'phases/plan.yaml'
								},
								params: { max_rounds: 5 },
								exits: { rejected: 'plan' },
								graph: {
									nodes: [
										{ id: 'run', kind: 'step' },
										{ id: 'approve', kind: 'wait' }
									],
									edges: [{ from: 'run', to: 'approve' }]
								}
							},
							{ id: 'lint', use: '.enzarb/phases/lint', origin: { layer: 'repo' } }
						]
					}
				],
				lint: [{ level: 'error', message: 'unknown column', path: 'board.yaml' }]
			}
		});
		await expect.element(page.getByText('unknown column')).toBeVisible();
		await expect.element(page.getByText('issues', { exact: true })).toBeVisible();
		await expect.element(page.getByText('max_rounds=5')).toBeVisible();
		await expect.element(page.getByText('rejected → plan')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'library@abcdef1' }))
			.toHaveAttribute(
				'href',
				'https://github.com/enzarb/workflows/blob/abcdef1234/phases/plan.yaml'
			);
		await expect.element(page.getByText('repo', { exact: true })).toBeVisible();
		await page.getByRole('button', { name: 'Show internals' }).click();
		await expect.element(page.getByTestId('phase-graph')).toBeInTheDocument();
	});
});

describe('SecretsForm', () => {
	it('lists names, saves and deletes', async () => {
		const onset = vi.fn(async () => {});
		const ondelete = vi.fn(async () => {});
		render(SecretsForm, {
			secrets: [{ name: 'NPM_TOKEN', updatedAt: '2026-10-16T00:00:00.000Z' }],
			onset,
			ondelete
		});
		await expect.element(page.getByText('NPM_TOKEN', { exact: true })).toBeVisible();
		const save = page.getByRole('button', { name: 'Save secret' });
		await page.getByLabelText('Name').fill('bad name');
		await page.getByLabelText('Value').fill('v');
		await expect.element(save).toBeDisabled();
		await page.getByLabelText('Name').fill('API_KEY');
		await userEvent.click(save);
		expect(onset).toHaveBeenCalledWith('API_KEY', 'v');
		await expect.element(page.getByLabelText('Value')).toHaveValue('');
		await page.getByRole('button', { name: 'Delete NPM_TOKEN' }).click();
		expect(ondelete).toHaveBeenCalledWith('NPM_TOKEN');
	});
});
