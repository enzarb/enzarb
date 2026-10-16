import type { TaskRow } from '#lib/tasks.ts';

/** A projection row for tests. */
export const row = (over: Partial<TaskRow> = {}): TaskRow => ({
	id: 1,
	project: 'web',
	repo: 'acme/web',
	workflowRef: 'std/workflows/issue',
	title: 'Add search',
	subjects: [],
	status: 'waiting',
	endStatus: null,
	phase: 'plan',
	node: 'approve',
	column: 'review',
	waitingOn: 'human',
	reason: null,
	text: null,
	actions: [],
	artifacts: [],
	needsYou: false,
	costUsd: null,
	createdAt: '2026-10-16T00:00:00.000Z',
	updatedAt: '2026-10-16T00:00:00.000Z',
	endedAt: null,
	...over
});
