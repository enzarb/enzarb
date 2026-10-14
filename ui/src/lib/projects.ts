import * as v from 'valibot';
import type { PendingChange, Project, ProjectView, Repository } from './project-types.ts';

/** `sb-<name>-workspace` must fit in 63 characters (contracts §2, ADR-0013). */
export const MAX_NAME_LENGTH = 50;

const NAME_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/;
const KEY_CHARS_RE = /^[A-Za-z0-9._-]+$/;

/**
 * Repo key `<connector>:<owner>/<repo>`, lowercase. Mirrors the operator's
 * `RepoKey::from_connector_url` (enzarb-sandbox-operator, crates/enzarb-crds/src/repo.rs): the
 * URL must start with `https://`, the host is ignored, trailing `/` are trimmed and one `.git`
 * suffix is stripped. Undefined when the operator would reject it.
 */
export function normalizeRepoKey(connector: string, url: string): string | undefined {
	const rest = url.trim().startsWith('https://') ? url.trim().slice('https://'.length) : undefined;
	if (rest === undefined) return undefined;
	const slash = rest.indexOf('/');
	if (slash < 0) return undefined;
	let path = rest.slice(slash + 1).replace(/\/+$/, '');
	if (path.endsWith('.git')) path = path.slice(0, -'.git'.length);
	const parts = path.split('/');
	if (parts.length !== 2) return undefined;
	const [owner, repo] = parts;
	if (![connector, owner, repo].every((x) => x && KEY_CHARS_RE.test(x))) return undefined;
	return `${connector}:${owner}/${repo}`.toLowerCase();
}

/** Longest Project display name (creation and rename). */
export const DISPLAY_NAME_MAX = 100;

export const NameSchema = v.pipe(
	v.string(),
	v.trim(),
	v.minLength(1, 'Name is required'),
	v.maxLength(MAX_NAME_LENGTH, `Name must be at most ${MAX_NAME_LENGTH} characters`),
	v.regex(NAME_RE, 'Use lowercase letters, digits and "-"; start and end with a letter or digit')
);

/** What sandbox-init runs at once when `spec.maxRuns` is unset (UI mirror, not read from the cluster). */
export const DEFAULT_MAX_RUNS = 2;
export const MIN_MAX_RUNS = 1;
export const MAX_MAX_RUNS = 16;

/** Concurrent agent runs per sandbox: a whole number from 1 to 16. */
export const MaxRunsSchema = v.pipe(
	v.number('Enter a whole number'),
	v.integer('Enter a whole number'),
	v.minValue(MIN_MAX_RUNS, `Must be at least ${MIN_MAX_RUNS}`),
	v.maxValue(MAX_MAX_RUNS, `Must be at most ${MAX_MAX_RUNS}`)
);

/** Build the `Project` object for a validated form (contracts §2; owner = GitHub login). */
export interface NewProject {
	name: string;
	displayName: string;
	repositories: Repository[];
	triggers: { issues: boolean; changeRequests: boolean; mentions: boolean };
	/** Omitted from the spec when undefined, so the sandbox default applies. */
	maxRuns?: number;
}

export function buildProject(input: NewProject, owner: string): Project {
	return {
		apiVersion: 'enzarb.dev/v1alpha1',
		kind: 'Project',
		metadata: { name: input.name },
		spec: {
			owner,
			displayName: input.displayName || input.name,
			repositories: input.repositories,
			runMode: 'worktree',
			triggers: input.triggers,
			...(input.maxRuns !== undefined ? { maxRuns: input.maxRuns } : {})
		}
	};
}

/**
 * `status.pendingChange`, or null when absent or malformed. Falls back to the
 * `SandboxChangePending` condition (True only while a change is pending) when the field is missing.
 */
export function toPendingChange(p: Project): PendingChange | null {
	const raw = p.status?.pendingChange;
	if (raw && typeof raw === 'object') {
		const { generation, since, reason } = raw;
		if (typeof since === 'string' && since && typeof reason === 'string') {
			return {
				generation: typeof generation === 'number' && Number.isFinite(generation) ? generation : 0,
				since,
				reason
			};
		}
	}
	const cond = p.status?.conditions?.find((c) => c.type === 'SandboxChangePending');
	if (cond?.status === 'True') {
		return {
			generation: 0,
			since: cond.lastTransitionTime ?? '',
			reason: cond.message || cond.reason || ''
		};
	}
	return null;
}

export type PendingStepId = 'queued' | 'waiting' | 'applying' | 'ready';

export interface PendingStep {
	id: PendingStepId;
	label: string;
	state: 'pending' | 'active' | 'done';
}

/**
 * Checklist for a queued sandbox change. `change` is the pending change (null once cleared);
 * `runningJobs` the jobs still running (null while not yet known); `applied` is true when a change
 * just cleared (all done).
 */
export function pendingChangeSteps(
	change: PendingChange | null | undefined,
	runningJobs: number | null,
	applied = false
): PendingStep[] {
	const unknown = !!change && runningJobs === null;
	const draining = !!change && runningJobs !== null && runningJobs > 0;
	const applying = !!change && runningJobs !== null && runningJobs <= 0;
	const done = applied && !change;
	const state = (isDone: boolean, isActive: boolean): PendingStep['state'] =>
		isDone || done ? 'done' : isActive ? 'active' : 'pending';
	return [
		{ id: 'queued', label: 'Change queued', state: state(!!change, false) },
		{
			id: 'waiting',
			label: unknown
				? 'Checking running jobs'
				: draining
					? 'Waiting for running jobs to finish'
					: 'Running jobs finished',
			state: state(applying, draining || unknown)
		},
		{ id: 'applying', label: 'Applying', state: state(false, applying) },
		{ id: 'ready', label: 'Sandbox change applied', state: state(false, false) }
	];
}

export function toView(p: Project): ProjectView {
	const conditions = p.status?.conditions ?? [];
	return {
		name: p.metadata.name,
		displayName: p.spec.displayName || p.metadata.name,
		namespace: p.status?.namespace ?? null,
		sandboxName: p.status?.sandbox?.name ?? null,
		repositories: p.spec.repositories ?? [],
		triggers: {
			issues: p.spec.triggers?.issues ?? true,
			changeRequests: p.spec.triggers?.changeRequests ?? true,
			mentions: p.spec.triggers?.mentions ?? true
		},
		maxRuns: p.spec.maxRuns,
		sandboxSettings: {
			cpu: p.spec.sandbox?.cpu ?? null,
			memory: p.spec.sandbox?.memory ?? null,
			storage: p.spec.sandbox?.storage ?? null
		},
		conditions,
		ready: conditions.some((c) => c.type === 'Ready' && c.status === 'True'),
		generation: p.metadata.generation ?? null,
		observedGeneration: p.status?.observedGeneration ?? null,
		pendingChange: toPendingChange(p)
	};
}
