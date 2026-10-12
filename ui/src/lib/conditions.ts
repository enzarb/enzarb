import type { Condition } from './project-types.ts';

/**
 * Boot order of Project conditions (contracts §2, ADR-0013). Condition types not listed here
 * (e.g. the old TenantReady/NamespaceReady) are ignored.
 */
export const BOOT_STEPS = [
	{ type: 'ResourcesReady', label: 'Resources' },
	{ type: 'SandboxReady', label: 'Sandbox' },
	{ type: 'Ready', label: 'Project ready' }
] as const;

export type StepState = 'pending' | 'in-progress' | 'done' | 'failed';

export interface ChecklistStep {
	type: string;
	label: string;
	state: StepState;
	message: string;
}

/**
 * Map `status.conditions` to the checklist:
 * - done: condition is True;
 * - failed: condition is False with a reason ending in Failed/Failure/Error;
 * - in-progress: the first step that is neither done nor failed (only once the operator has
 *   reported any condition);
 * - pending: every later step, and all steps before the operator reports anything.
 */
export function buildChecklist(conditions: Condition[] | undefined | null): ChecklistStep[] {
	const byType = new Map((conditions ?? []).map((c) => [c.type, c]));
	let active = (conditions?.length ?? 0) > 0; // no unfinished step before this one yet
	return BOOT_STEPS.map(({ type, label }) => {
		const c = byType.get(type);
		const message = c?.message ?? '';
		if (c?.status === 'True') return { type, label, state: 'done', message };
		if (isFailure(c)) {
			active = false;
			return { type, label, state: 'failed', message: message || c?.reason || '' };
		}
		if (active) {
			active = false;
			return { type, label, state: 'in-progress', message };
		}
		return { type, label, state: 'pending', message: '' };
	});
}

/**
 * Contracts §2 failed reasons: the suffixes below (covers `NotOwnedError`) and the named
 * terminal reason `InvalidSpec`. Other False reasons, e.g. `Terminating`, are in progress.
 */
const FAILURE_REASON_RE = /(Failed|Failure|Error)$/;

function isFailure(c: Condition | undefined): boolean {
	return (
		!!c &&
		c.status === 'False' &&
		(c.reason === 'InvalidSpec' || FAILURE_REASON_RE.test(c.reason ?? ''))
	);
}

/** Overall badge for list pages. */
export function overallState(conditions: Condition[] | undefined | null): StepState {
	const steps = buildChecklist(conditions);
	if (steps.some((s) => s.state === 'failed')) return 'failed';
	if (steps.every((s) => s.state === 'done')) return 'done';
	return conditions?.length ? 'in-progress' : 'pending';
}

/** True once the operator will not make further progress without a change: failed or invalid. */
export function isTerminalFailure(conditions: Condition[] | undefined | null): boolean {
	return overallState(conditions) === 'failed';
}

export interface SandboxChangeStep {
	id: 'saving' | 'queued' | 'applying' | 'ready' | 'storage';
	label: string;
	state: StepState;
	message: string;
}

/** Fields of a ProjectView that the change checklist reads. */
export interface SandboxChangeSource {
	conditions: Condition[];
	pendingChange?: unknown;
	generation: number | null;
	observedGeneration: number | null;
}

const condition = (conditions: Condition[], type: string) =>
	conditions.find((c) => c.type === type);

const QUEUED_LABEL = 'Queued (waiting for running jobs to finish)';

/**
 * Checklist for a saved pod-shape change (drain-first, operator v0.7.0+):
 * Saving (remote call in flight) → Queued (`status.pendingChange` / `SandboxChangePending` True,
 * waiting for running jobs) → Applying (nothing pending, sandbox not yet ready for this
 * generation) → Ready (`Ready` True and `observedGeneration` caught up).
 * A False `SandboxChangePending` or `SandboxReady` condition with a failure reason fails its step.
 */
export function buildSandboxChangeSteps(
	project: SandboxChangeSource,
	saving: boolean
): SandboxChangeStep[] {
	const { conditions } = project;
	const step = (
		id: SandboxChangeStep['id'],
		label: string,
		state: StepState,
		message = ''
	): SandboxChangeStep => ({ id, label, state, message });

	if (saving) {
		return [
			step('saving', 'Saving', 'in-progress'),
			step('queued', QUEUED_LABEL, 'pending'),
			step('applying', 'Applying', 'pending'),
			step('ready', 'Ready', 'pending')
		];
	}
	const pendingCond = condition(conditions, 'SandboxChangePending');
	const sandbox = condition(conditions, 'SandboxReady');
	const ready = condition(conditions, 'Ready');
	const queued = !!project.pendingChange || pendingCond?.status === 'True';
	const queueFailed = isFailure(pendingCond);
	const caughtUp =
		project.generation === null || (project.observedGeneration ?? -1) >= project.generation;
	const isReady = ready?.status === 'True' && sandbox?.status !== 'False' && caughtUp;
	const applyFailed = !queued && !queueFailed && isFailure(sandbox);

	const queuedState: StepState = queueFailed ? 'failed' : queued ? 'in-progress' : 'done';
	const applyingState: StepState = applyFailed
		? 'failed'
		: queued || queueFailed
			? 'pending'
			: isReady
				? 'done'
				: 'in-progress';
	return [
		step('saving', 'Saving', 'done'),
		step(
			'queued',
			QUEUED_LABEL,
			queuedState,
			queueFailed ? pendingCond?.message || pendingCond?.reason || '' : ''
		),
		step(
			'applying',
			'Applying',
			applyingState,
			applyFailed ? sandbox?.message || sandbox?.reason || '' : ''
		),
		step('ready', 'Ready', applyingState === 'done' ? 'done' : 'pending')
	];
}

/**
 * Storage growth is tracked apart from the queued change: `ResourcesReady` with reason
 * `WorkspaceResizing` is in progress, `WorkspaceResizeFailed` is failed. Null otherwise.
 */
export function buildStorageResizeStep(
	conditions: Condition[] | undefined | null
): SandboxChangeStep | null {
	const c = condition(conditions ?? [], 'ResourcesReady');
	const label = 'Growing workspace storage';
	if (c?.reason === 'WorkspaceResizing') {
		return { id: 'storage', label, state: 'in-progress', message: c.message ?? '' };
	}
	if (c?.reason === 'WorkspaceResizeFailed') {
		return { id: 'storage', label, state: 'failed', message: c.message || c.reason };
	}
	return null;
}
