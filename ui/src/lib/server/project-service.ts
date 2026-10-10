import type { Project, ProjectView } from '#lib/project-types.ts';
import { toView } from '#lib/projects.ts';
import { KubeError, type ProjectStore } from './kube.ts';

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
		}
	};
}
