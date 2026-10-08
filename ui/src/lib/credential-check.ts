/** Checklist for asking the sandbox whether the Claude credential is set. */
export const CREDENTIAL_STEPS = [
	{ id: 'token', label: 'Session token' },
	{ id: 'connecting', label: 'Connecting to sandbox' },
	{ id: 'status', label: 'Asking for credential status' }
] as const;

export type CredentialStepId = (typeof CREDENTIAL_STEPS)[number]['id'];
export type CredentialStepState = 'pending' | 'in-progress' | 'done' | 'failed';

export interface CredentialStep {
	id: CredentialStepId;
	label: string;
	state: CredentialStepState;
}

export type CredentialCheck =
	| { phase: 'checking'; done: number }
	/** `configured` is the sandbox's answer. */
	| { phase: 'answered'; configured: boolean }
	/** The sandbox could not answer; fall back to the cached flag. */
	| { phase: 'unavailable'; step: number; reason: string };

export const startCheck: CredentialCheck = { phase: 'checking', done: 0 };

/** One more step finished while checking. */
export function advanceCheck(c: CredentialCheck): CredentialCheck {
	return c.phase === 'checking' ? { phase: 'checking', done: c.done + 1 } : c;
}

/** The running step failed; keeps the failed step's index for the checklist. */
export function failCheck(c: CredentialCheck, e: unknown): CredentialCheck {
	if (c.phase !== 'checking') return c;
	return { phase: 'unavailable', step: c.done, reason: describeCredentialError(e) };
}

export function buildCredentialSteps(check: CredentialCheck): CredentialStep[] {
	return CREDENTIAL_STEPS.map((s, i) => {
		let state: CredentialStepState;
		if (check.phase === 'answered') state = 'done';
		else if (check.phase === 'unavailable')
			state = i < check.step ? 'done' : i === check.step ? 'failed' : 'pending';
		else state = i < check.done ? 'done' : i === check.done ? 'in-progress' : 'pending';
		return { id: s.id, label: s.label, state };
	});
}

/** True when an error means the sandbox does not support `credential_status`. */
export function isUnimplemented(e: unknown): boolean {
	const remote = (e as { details?: { remote?: { code?: string } } } | null)?.details?.remote;
	return remote?.code === 'unimplemented';
}

export function describeCredentialError(e: unknown): string {
	if (isUnimplemented(e)) return 'The sandbox does not report credential status yet';
	return e instanceof Error ? e.message : String(e);
}
