import type { ProjectView } from '#lib/project-types.ts';
import type { ProjectStore } from './kube.ts';
import { projectService } from './project-service.ts';

export const PROJECT_CACHE_TTL_MS = 15_000;

interface Entry {
	at: number;
	projects: Promise<ProjectView[]>;
}

/**
 * Per-user cache of the owned-project list, so each remote call does not list every Project CR.
 * Short TTL; create/delete invalidate. Failed lookups are not cached.
 */
export function createProjectCache(ttlMs = PROJECT_CACHE_TTL_MS, now: () => number = Date.now) {
	const entries = new Map<string, Entry>();
	return {
		list(store: ProjectStore, owner: string): Promise<ProjectView[]> {
			const hit = entries.get(owner);
			if (hit && now() - hit.at < ttlMs) return hit.projects;
			const projects = projectService(store, owner).list();
			const entry = { at: now(), projects };
			entries.set(owner, entry);
			projects.catch(() => {
				if (entries.get(owner) === entry) entries.delete(owner);
			});
			return projects;
		},
		/** Stores a freshly listed result (a caller that had to list anyway). */
		put(owner: string, projects: ProjectView[]): void {
			entries.set(owner, { at: now(), projects: Promise.resolve(projects) });
		},
		invalidate(owner: string): void {
			entries.delete(owner);
		}
	};
}

export type ProjectCache = ReturnType<typeof createProjectCache>;

export const projectCache = createProjectCache();
