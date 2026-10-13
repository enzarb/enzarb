/** Shown after saving a pod-shape setting (max runs, CPU, memory) and in the pending-change banner. */
export const POD_SHAPE_QUEUED_COPY =
	'The change is queued: it takes effect once currently running jobs finish. Running jobs are not interrupted.';

/** Confirmation text before saving a pod-shape setting (drain-first, operator v0.7.0+). */
export const POD_SHAPE_CONFIRM_COPY =
	'The change is queued until running jobs finish. New jobs wait until it is applied, then the sandbox is updated.';
