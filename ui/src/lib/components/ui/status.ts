import type { StepState } from '#lib/conditions.ts';
import { stateTone as workTone } from '#lib/work-items.ts';

/** Visual tone shared by Badge, Notice and Toast. */
export type Tone = 'neutral' | 'accent' | 'success' | 'warning' | 'danger' | 'info';

export interface Status {
	tone: Tone;
	hollow?: boolean;
}

/** Project condition roll-up (StatusBadge). */
export const stepStatus: Record<StepState, Status> = {
	pending: { tone: 'neutral', hollow: true },
	'in-progress': { tone: 'info' },
	done: { tone: 'success' },
	failed: { tone: 'danger' }
};

/** Work item state (WorkStateChip), from the existing `stateTone` classification. */
export function workStatus(state: string): Status {
	switch (workTone(state)) {
		case 'failed':
			return { tone: 'danger' };
		case 'done':
			return { tone: 'success' };
		case 'waiting':
			return { tone: 'warning', hollow: true };
		default:
			return { tone: 'accent' };
	}
}

/** Run (job) result. */
export function jobStatus(status: string): Status {
	switch (status) {
		case 'succeeded':
			return { tone: 'success' };
		case 'failed':
			return { tone: 'danger' };
		case 'running':
			return { tone: 'info' };
		default:
			return { tone: 'neutral', hollow: true };
	}
}
