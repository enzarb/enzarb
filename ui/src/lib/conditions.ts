import type { Condition } from './project-types.ts';

/** Boot order of Project conditions (contracts §2). */
export const BOOT_STEPS = [
	{ type: 'TenantReady', label: 'Tenant' },
	{ type: 'NamespaceReady', label: 'Namespace' },
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

/** Contracts §2 failure-reason suffixes. */
const FAILURE_REASON_RE = /(Failed|Failure|Error)$/;

function isFailure(c: Condition | undefined): boolean {
	return !!c && c.status === 'False' && FAILURE_REASON_RE.test(c.reason ?? '');
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
	return (
		overallState(conditions) === 'failed' ||
		!!conditions?.some((c) => c.status === 'False' && c.reason === 'InvalidSpec')
	);
}
