import type { Project, ProjectView } from '#lib/project-types.ts';
import { toView } from '#lib/projects.ts';
import {
	buildSandboxPatch,
	validateStorageGrowth,
	type SandboxResourcesInput
} from '#lib/sandbox-resources.ts';
import { KubeError, type ProjectStore } from './kube.ts';

/** Operator contract (ADR-0023): releases the pending sandbox roll with this exact generation. */
export const ANNOTATION_ROLL_NOW_GENERATION = 'enzarb.dev/roll-now-generation';

/** Owner-scoped project operations; everything a remote function does goes through here. */
export function projectService(store: ProjectStore, owner: string) {
	const mine = (p: Project | null): p is Project => !!p && p.spec.owner === owner;
	return {
		async list(): Promise<ProjectView[]> {
			return (await store.list())
				.filter(mine)
				.map(toView)
				.sort((a, b) => a.name.localeCompare(b.name));
		},
		/** Null for missing projects and for projects owned by someone else. */
		async get(name: string): Promise<ProjectView | null> {
			const p = await store.get(name);
			return mine(p) ? toView(p) : null;
		},
		async delete(name: string): Promise<void> {
			const p = await store.get(name);
			if (!mine(p)) throw new KubeError('not-found', 'Project not found');
			await store.delete(name);
		},
		/** Sets `spec.maxRuns`; null removes it (sandbox default). Not-found for non-owners. */
		async setMaxRuns(name: string, maxRuns: number | null): Promise<void> {
			const p = await store.get(name);
			if (!mine(p)) throw new KubeError('not-found', 'Project not found');
			await store.update(name, { maxRuns });
		},
		/**
		 * "Roll now": asks the operator to restart the sandbox pod for the pending change the user saw.
		 * Not-found for non-owners; `conflict` when nothing is pending or the generation moved on.
		 */
		async rollNow(name: string, generation: number): Promise<void> {
			const p = await store.get(name);
			if (!mine(p)) throw new KubeError('not-found', 'Project not found');
			if (p.status?.pendingChange?.generation !== generation)
				throw new KubeError('conflict', 'The pending change moved on; review it and try again');
			await store.annotate(name, { [ANNOTATION_ROLL_NOW_GENERATION]: String(generation) });
		},
		/**
		 * Sets `spec.sandbox.{cpu,memory,storage}`; null cpu/memory clear them. Storage may only grow
		 * (checked against the stored spec). Not-found for non-owners, `invalid` for a shrink.
		 */
		async setSandboxResources(name: string, input: SandboxResourcesInput): Promise<void> {
			const p = await store.get(name);
			if (!mine(p)) throw new KubeError('not-found', 'Project not found');
			if (input.storage) {
				const err = validateStorageGrowth(p.spec.sandbox?.storage, input.storage);
				if (err) throw new KubeError('invalid', err);
			}
			await store.update(name, buildSandboxPatch(input));
		}
	};
}
