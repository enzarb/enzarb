import { describe, expect, it } from 'vitest';
import type { Project } from '#lib/project-types.ts';
import { buildProject } from '#lib/projects.ts';
import type { ProjectStore } from './kube.ts';
import { createProjectCache } from './project-cache.ts';

function countingStore(items: Project[]) {
	let lists = 0;
	const store: ProjectStore = {
		async list() {
			lists += 1;
			return items;
		},
		async get() {
			return null;
		},
		async create(p) {
			return p;
		},
		async delete() {}
	};
	return { store, lists: () => lists };
}

const proj = (name: string, owner: string) =>
	buildProject(
		{
			name,
			displayName: '',
			repositories: [],
			triggers: { issues: false, changeRequests: false, mentions: false }
		},
		owner
	);

describe('project cache', () => {
	it('serves repeat lookups from cache within the TTL, per user', async () => {
		const { store, lists } = countingStore([proj('a', 'alice'), proj('b', 'bob')]);
		let t = 0;
		const cache = createProjectCache(15_000, () => t);
		expect((await cache.list(store, 'alice')).map((p) => p.name)).toEqual(['a']);
		t = 14_999;
		await cache.list(store, 'alice');
		expect(lists()).toBe(1);
		expect((await cache.list(store, 'bob')).map((p) => p.name)).toEqual(['b']);
		expect(lists()).toBe(2);
	});

	it('expires after the TTL', async () => {
		const { store, lists } = countingStore([proj('a', 'alice')]);
		let t = 0;
		const cache = createProjectCache(15_000, () => t);
		await cache.list(store, 'alice');
		t = 15_000;
		await cache.list(store, 'alice');
		expect(lists()).toBe(2);
	});

	it('invalidate drops only that user', async () => {
		const { store, lists } = countingStore([proj('a', 'alice'), proj('b', 'bob')]);
		const cache = createProjectCache(15_000, () => 0);
		await cache.list(store, 'alice');
		await cache.list(store, 'bob');
		cache.invalidate('alice');
		await cache.list(store, 'alice');
		await cache.list(store, 'bob');
		expect(lists()).toBe(3);
	});

	it('does not cache failures', async () => {
		let fail = true;
		const store: ProjectStore = {
			async list() {
				if (fail) throw new Error('boom');
				return [];
			},
			async get() {
				return null;
			},
			async create(p) {
				return p;
			},
			async delete() {}
		};
		const cache = createProjectCache(15_000, () => 0);
		await expect(cache.list(store, 'alice')).rejects.toThrow('boom');
		fail = false;
		await expect(cache.list(store, 'alice')).resolves.toEqual([]);
	});
});
