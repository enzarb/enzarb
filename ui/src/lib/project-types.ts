/** Subset of the `Project` CRD (contracts §2) that the UI reads and writes. */
export interface Condition {
	type: string;
	status: 'True' | 'False' | 'Unknown' | string;
	reason?: string;
	message?: string;
	lastTransitionTime?: string;
}

export interface Repository {
	url: string;
	connector: string;
	defaultBranch: string;
}

export interface ProjectSpec {
	owner: string;
	displayName?: string;
	repositories?: Repository[];
	runMode?: 'worktree' | 'fresh';
	triggers?: { issues?: boolean; changeRequests?: boolean; mentions?: boolean };
	/** Concurrent agent runs, 1-16; the sandbox default applies when unset. */
	maxRuns?: number;
	/** Pod shape (operator v0.8.0): null/unset cpu and memory mean no limit; storage defaults to 20Gi. */
	sandbox?: { name?: string; cpu?: number | null; memory?: string | null; storage?: string | null };
}

/** `status.pendingChange` (operator v0.8.0): a queued pod-shape change, absent when nothing is pending. */
export interface PendingChange {
	/** Counter local to the pending episode; a newer change bumps it. */
	generation: number;
	/** RFC 3339 timestamp. */
	since: string;
	reason: string;
}

export interface ProjectStatus {
	pendingChange?: Partial<PendingChange> | null;
	namespace?: string | null;
	sandbox?: { name?: string | null } | null;
	observedGeneration?: number | null;
	conditions?: Condition[];
}

export interface Project {
	apiVersion: 'enzarb.dev/v1alpha1';
	kind: 'Project';
	metadata: { name: string; creationTimestamp?: string; generation?: number };
	spec: ProjectSpec;
	status?: ProjectStatus | null;
}

/** What remote functions return to the browser. */
export interface ProjectView {
	name: string;
	displayName: string;
	namespace: string | null;
	sandboxName: string | null;
	repositories: Repository[];
	triggers: { issues: boolean; changeRequests: boolean; mentions: boolean };
	/** Raw `spec.maxRuns`; undefined when unset (the sandbox default applies). */
	maxRuns?: number;
	/** Raw `spec.sandbox.{cpu,memory,storage}`; null when unset. */
	sandboxSettings: { cpu: number | null; memory: string | null; storage: string | null };
	conditions: Condition[];
	ready: boolean;
	/** `metadata.generation` and `status.observedGeneration`: has the operator seen the latest edit. */
	generation: number | null;
	observedGeneration: number | null;
	pendingChange?: PendingChange | null;
}
