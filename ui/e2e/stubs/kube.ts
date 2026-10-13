// Stand-in for src/lib/server/kube.ts in the e2e web server (see vite.e2e.config.ts): the same
// surface, backed by memory, holding one ready project owned by the e2e user.
import type { Project } from '#lib/project-types.ts';
import { KubeError, type ProjectStore, type SpecPatch } from '../../src/lib/server/kube.ts';
import { E2E_PROJECT, E2E_USER } from '../constants.ts';

export { KubeError, type ProjectStore, type SpecPatch };

function seedProject(): Project {
	return {
		apiVersion: 'enzarb.dev/v1alpha1',
		kind: 'Project',
		metadata: {
			name: E2E_PROJECT,
			creationTimestamp: '2026-01-01T00:00:00Z',
			generation: 1
		},
		spec: {
			owner: E2E_USER,
			displayName: 'E2E project',
			repositories: [
				{ url: 'https://github.com/e2e-org/app', connector: 'github', defaultBranch: 'main' }
			]
		},
		status: {
			namespace: `enzarb-${E2E_PROJECT}`,
			sandbox: { name: 'sandbox' },
			observedGeneration: 1,
			conditions: [{ type: 'Ready', status: 'True', reason: 'Ready' }]
		}
	};
}

export function createMemoryProjectStore(initial: Project[] = [seedProject()]): ProjectStore {
	const projects = new Map(initial.map((p) => [p.metadata.name, p]));
	const need = (name: string) => {
		const p = projects.get(name);
		if (!p) throw new KubeError('not-found', 'Project not found');
		return p;
	};
	return {
		async list() {
			return [...projects.values()];
		},
		async get(name) {
			return projects.get(name) ?? null;
		},
		async create(project) {
			if (projects.has(project.metadata.name)) {
				throw new KubeError('conflict', 'Project already exists');
			}
			projects.set(project.metadata.name, project);
			return project;
		},
		async delete(name) {
			need(name);
			projects.delete(name);
		},
		async update(name, spec) {
			const p = need(name);
			if (spec.maxRuns !== undefined) {
				if (spec.maxRuns === null) delete p.spec.maxRuns;
				else p.spec.maxRuns = spec.maxRuns;
			}
			if (spec.repositories !== undefined) p.spec.repositories = spec.repositories;
			return p;
		}
	};
}

let store: ProjectStore | undefined;

export function getProjectStore(): ProjectStore {
	return (store ??= createMemoryProjectStore());
}

export function setProjectStore(s: ProjectStore | undefined): void {
	store = s;
}
