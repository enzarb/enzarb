import { ApiException, CustomObjectsApi, KubeConfig } from '@kubernetes/client-node';
import type { Project } from '#lib/project-types.ts';

const GROUP = 'enzarb.dev';
const VERSION = 'v1alpha1';
const PLURAL = 'projects';

export class KubeError extends Error {
	constructor(
		readonly kind: 'not-found' | 'conflict' | 'other',
		message: string
	) {
		super(message);
	}
}

/** The only Kubernetes surface the UI needs; tests swap in a fake. */
export interface ProjectStore {
	list(): Promise<Project[]>;
	get(name: string): Promise<Project | null>;
	create(project: Project): Promise<Project>;
	delete(name: string): Promise<void>;
}

function wrap(e: unknown): never {
	if (e instanceof ApiException) {
		if (e.code === 404) throw new KubeError('not-found', 'Project not found');
		if (e.code === 409) throw new KubeError('conflict', 'Project already exists');
		throw new KubeError('other', `Kubernetes API error ${e.code}`);
	}
	throw e;
}

/** In-cluster config, falling back to `KUBECONFIG` / `~/.kube/config` (client-node's default loader). */
export function createProjectStore(kc = new KubeConfig()): ProjectStore {
	if (kc.getClusters().length === 0) kc.loadFromDefault();
	const api = kc.makeApiClient(CustomObjectsApi);
	const ref = { group: GROUP, version: VERSION, plural: PLURAL };
	return {
		async list() {
			try {
				const res = (await api.listClusterCustomObject(ref)) as { items?: Project[] };
				return res.items ?? [];
			} catch (e) {
				wrap(e);
			}
		},
		async get(name) {
			try {
				return (await api.getClusterCustomObject({ ...ref, name })) as Project;
			} catch (e) {
				if (e instanceof ApiException && e.code === 404) return null;
				wrap(e);
			}
		},
		async create(project) {
			try {
				return (await api.createClusterCustomObject({ ...ref, body: project })) as Project;
			} catch (e) {
				wrap(e);
			}
		},
		async delete(name) {
			try {
				await api.deleteClusterCustomObject({ ...ref, name });
			} catch (e) {
				wrap(e);
			}
		}
	};
}

let store: ProjectStore | undefined;

export function getProjectStore(): ProjectStore {
	return (store ??= createProjectStore());
}

/** Test hook. */
export function setProjectStore(s: ProjectStore | undefined): void {
	store = s;
}
