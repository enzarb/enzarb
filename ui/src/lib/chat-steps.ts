/** Connection checklist for the chat tab: token -> connect -> handshake -> start agent -> initialize -> session. */
export const CHAT_STEPS = [
	{ id: 'token', label: 'Session token' },
	{ id: 'connect', label: 'Connecting to sandbox' },
	{ id: 'handshake', label: 'Protocol handshake' },
	{ id: 'agent', label: 'Start agent' },
	{ id: 'initialize', label: 'Initialize' },
	{ id: 'session', label: 'Chat session' }
] as const;

export type ChatStepId = (typeof CHAT_STEPS)[number]['id'];
export type ChatStepState = 'pending' | 'active' | 'done' | 'error';

export interface ChatStep {
	id: ChatStepId;
	label: string;
	state: ChatStepState;
	message: string;
}

export interface ChatProgress {
	/** Index of the running step; CHAT_STEPS.length when all are done. */
	current: number;
	failure: { step: ChatStepId; message: string } | null;
}

export const initialChatProgress: ChatProgress = { current: 0, failure: null };

/** Shown while the sandbox tears down the previous agent (up to ~20 s); a static label, no spinner. */
export const STARTING_AGENT_LABEL = 'Starting agent…';

export function completeChatStep(p: ChatProgress, id: ChatStepId): ChatProgress {
	const i = CHAT_STEPS.findIndex((s) => s.id === id);
	if (p.failure || i < p.current) return p;
	return { ...p, current: i + 1 };
}

/** Fail the named step, or the running one when `id` is omitted. */
export function failChatStep(p: ChatProgress, message: string, id?: ChatStepId): ChatProgress {
	if (p.failure) return p;
	const step = id ?? CHAT_STEPS[Math.min(p.current, CHAT_STEPS.length - 1)].id;
	return { ...p, failure: { step, message } };
}

export function buildChatSteps(p: ChatProgress): ChatStep[] {
	return CHAT_STEPS.map((s, i) => {
		const failed = p.failure?.step === s.id;
		const state: ChatStepState = failed
			? 'error'
			: i < p.current
				? 'done'
				: i === p.current && !p.failure
					? 'active'
					: 'pending';
		const label = s.id === 'agent' && state === 'active' ? STARTING_AGENT_LABEL : s.label;
		return { id: s.id, label, state, message: failed ? p.failure!.message : '' };
	});
}
