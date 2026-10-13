/** Connection checklist for the sandbox terminal: token -> connecting -> handshake -> shell. */
export const TERMINAL_STEPS = [
	{ id: 'token', label: 'Session token' },
	{ id: 'connecting', label: 'Connecting to sandbox' },
	{ id: 'handshake', label: 'Protocol handshake' },
	{ id: 'shell', label: 'Shell' }
] as const;

export type TerminalStepId = (typeof TERMINAL_STEPS)[number]['id'];
export type TerminalStepState = 'pending' | 'in-progress' | 'done' | 'failed';

export interface TerminalStep {
	id: TerminalStepId;
	label: string;
	state: TerminalStepState;
	message: string;
}

export interface TerminalProgress {
	/** Index of the step currently running; TERMINAL_STEPS.length when all are done. */
	current: number;
	failure: { step: TerminalStepId; message: string } | null;
}

export const initialProgress: TerminalProgress = { current: 0, failure: null };

/** Mark `id` finished and start the next step. */
export function completeStep(p: TerminalProgress, id: TerminalStepId): TerminalProgress {
	const i = TERMINAL_STEPS.findIndex((s) => s.id === id);
	if (p.failure || i < p.current) return p;
	return { ...p, current: i + 1 };
}

/** Fail the step that is currently running. */
export function failCurrent(p: TerminalProgress, message: string): TerminalProgress {
	if (p.failure) return p;
	const step = TERMINAL_STEPS[Math.min(p.current, TERMINAL_STEPS.length - 1)].id;
	return { ...p, failure: { step, message } };
}

export function buildSteps(p: TerminalProgress): TerminalStep[] {
	return TERMINAL_STEPS.map((s, i) => {
		const failed = p.failure?.step === s.id;
		const state: TerminalStepState = failed
			? 'failed'
			: i < p.current
				? 'done'
				: i === p.current && !p.failure
					? 'in-progress'
					: 'pending';
		return { id: s.id, label: s.label, state, message: failed ? p.failure!.message : '' };
	});
}

/** Human text for a PTY CLOSE reason (`exit:<code>`, `signal:<n>`, or anything else). */
export function describeClose(reason: string | undefined | null): string {
	const exit = /^exit:(-?\d+)$/.exec(reason ?? '');
	if (exit) return exit[1] === '0' ? 'Shell exited' : `Shell exited with code ${exit[1]}`;
	const sig = /^signal:(\d+)$/.exec(reason ?? '');
	if (sig) return `Shell killed by signal ${sig[1]}`;
	return reason ? `Session closed: ${reason}` : 'Session closed';
}
