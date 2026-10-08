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

export interface SandboxSpec {
	runtimeClassName?: string;
	docker?: { enabled: boolean };
}

export interface ProjectSpec {
	owner: string;
	displayName?: string;
	repositories?: Repository[];
	runMode?: 'worktree' | 'fresh';
	triggers?: { issues?: boolean; changeRequests?: boolean; mentions?: boolean };
	sandbox?: SandboxSpec;
}

export interface ProjectStatus {
	namespace?: string | null;
	observedGeneration?: number | null;
	conditions?: Condition[];
}

export interface Project {
	apiVersion: 'enzarb.dev/v1alpha1';
	kind: 'Project';
	metadata: { name: string; creationTimestamp?: string };
	spec: ProjectSpec;
	status?: ProjectStatus | null;
}

/** What remote functions return to the browser. */
export interface ProjectView {
	name: string;
	displayName: string;
	namespace: string | null;
	repositories: Repository[];
	triggers: { issues: boolean; changeRequests: boolean; mentions: boolean };
	conditions: Condition[];
	ready: boolean;
}
