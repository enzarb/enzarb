import { beforeEach, describe, expect, it } from 'vitest';
import { KubeError, type ProjectStore } from './kube.ts';
import { projectService } from './project-service.ts';
import type { Project } from '#lib/project-types.ts';
import { CreateProjectSchema } from '#lib/projects.ts';
import * as v from 'valibot';

function fakeStore(): ProjectStore & { items: Map<string, Project> } {
	const items = new Map<string, Project>();
	return {
		items,
		async list() {
			return [...items.values()];
		},
		async get(name) {
			return items.get(name) ?? null;
		},
		async create(p) {
			if (items.has(p.metadata.name)) throw new KubeError('conflict', 'exists');
			items.set(p.metadata.name, p);
			return p;
		},
		async delete(name) {
			if (!items.delete(name)) throw new KubeError('not-found', 'nope');
		}
	};
}

const input = (name: string) => v.parse(CreateProjectSchema, { name });

describe('projectService', () => {
	let store: ReturnType<typeof fakeStore>;
	beforeEach(async () => {
		store = fakeStore();
		await projectService(store, 'alice').create(input('b-proj'));
		await projectService(store, 'alice').create(input('a-proj'));
		await projectService(store, 'bob').create(input('bobs'));
	});

	it('lists only the caller’s projects, sorted', async () => {
		expect((await projectService(store, 'alice').list()).map((p) => p.name)).toEqual([
			'a-proj',
			'b-proj'
		]);
	});
	it('creates with owner = login', async () => {
		expect(store.items.get('bobs')?.spec.owner).toBe('bob');
	});
	it('rejects a duplicate name with a conflict', async () => {
		await expect(projectService(store, 'alice').create(input('a-proj'))).rejects.toMatchObject({
			kind: 'conflict'
		});
	});
	it('hides other owners’ projects on get and delete', async () => {
		const alice = projectService(store, 'alice');
		expect(await alice.get('bobs')).toBeNull();
		await expect(alice.delete('bobs')).rejects.toMatchObject({ kind: 'not-found' });
		expect(store.items.has('bobs')).toBe(true);
	});
	it('deletes own projects', async () => {
		await projectService(store, 'alice').delete('a-proj');
		expect(store.items.has('a-proj')).toBe(false);
	});
});
