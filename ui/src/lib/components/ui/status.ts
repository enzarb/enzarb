import type { StepState } from '#lib/conditions.ts';

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
