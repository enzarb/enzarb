/** Shown in the pending-change banner while a pod-shape setting (max runs, CPU, memory) is queued. */
export const POD_SHAPE_QUEUED_COPY =
	'The change is queued: it takes effect once currently running jobs finish. Running jobs are not interrupted.';

/** Shown on the settings form while an unsaved edit would trigger a drain-first change. */
export const POD_SHAPE_PRESAVE_COPY = 'Saving restarts the sandbox after the running jobs finish.';

/** Milliseconds until the drain `deadline` (RFC 3339), or null when absent or invalid. */
export function remainingDrain(deadline: string | undefined, now: number): number | null {
	if (!deadline) return null;
	const at = Date.parse(deadline);
	return Number.isNaN(at) ? null : at - now;
}

/** "forced in 24m", "forced in 1h 5m", "forced in 40s"; "any moment" once the deadline passed. */
export function formatRemainingDrain(ms: number): string {
	if (ms <= 0) return 'any moment';
	const s = Math.ceil(ms / 1000);
	if (s < 60) return `forced in ${s}s`;
	const m = Math.ceil(s / 60);
	if (m < 60) return `forced in ${m}m`;
	const h = Math.floor(m / 60);
	const rest = m % 60;
	return `forced in ${h}h${rest ? ` ${rest}m` : ''}`;
}
