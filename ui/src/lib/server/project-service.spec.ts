import { beforeEach, describe, expect, it, vi } from 'vitest';
import { KubeError, type ProjectStore } from './kube.ts';
import { projectService } from './project-service.ts';
import type { Project } from '#lib/project-types.ts';
import { buildProject } from '#lib/projects.ts';

/** RFC 7386 JSON merge patch: objects merge recursively, `null` removes the key. */
function mergePatch(target: unknown, patch: unknown): unknown {
	if (patch === null || typeof patch !== 'object' || Array.isArray(patch)) return patch;
	const out: Record<string, unknown> = { ...((target as Record<string, unknown>) ?? {}) };
	for (const [k, val] of Object.entries(patch)) {
		if (val === null) delete out[k];
		else out[k] = mergePatch(out[k], val);
	}
	return out;
}

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
		async update(name, spec) {
			const p = items.get(name);
			if (!p) throw new KubeError('not-found', 'nope');
			const next = { ...p, spec: mergePatch(p.spec, spec) as Project['spec'] };
			items.set(name, next);
			return next;
		},
		async delete(name) {
			if (!items.delete(name)) throw new KubeError('not-found', 'nope');
		}
	};
}

const seed = (store: ProjectStore, name: string, owner: string) =>
	store.create(
		buildProject(
			{
				name,
				displayName: '',
				repositories: [],
				triggers: { issues: false, changeRequests: false, mentions: false }
			},
			owner
		)
	);

describe('projectService', () => {
	let store: ReturnType<typeof fakeStore>;
	beforeEach(async () => {
		store = fakeStore();
		await seed(store, 'b-proj', 'alice');
		await seed(store, 'a-proj', 'alice');
		await seed(store, 'bobs', 'bob');
	});

	it('lists only the caller’s projects, sorted', async () => {
		expect((await projectService(store, 'alice').list()).map((p) => p.name)).toEqual([
			'a-proj',
			'b-proj'
		]);
	});
	it('hides other owners’ projects on get and delete', async () => {
		const alice = projectService(store, 'alice');
		expect(await alice.get('bobs')).toBeNull();
		await expect(alice.delete('bobs')).rejects.toMatchObject({ kind: 'not-found' });
		expect(store.items.has('bobs')).toBe(true);
	});
	it('sets and clears maxRuns on own projects', async () => {
		const alice = projectService(store, 'alice');
		await alice.setMaxRuns('a-proj', 6);
		expect((await alice.get('a-proj'))?.maxRuns).toBe(6);
		await alice.setMaxRuns('a-proj', null);
		expect((await alice.get('a-proj'))?.maxRuns).toBeUndefined();
	});
	it('does not patch other owners’ or missing projects', async () => {
		const spy = vi.spyOn(store, 'update');
		const alice = projectService(store, 'alice');
		await expect(alice.setMaxRuns('bobs', 4)).rejects.toMatchObject({ kind: 'not-found' });
		await expect(alice.setMaxRuns('ghost', 4)).rejects.toMatchObject({ kind: 'not-found' });
		expect(spy).not.toHaveBeenCalled();
	});
	describe('setSandboxResources', () => {
		it('patches only spec.sandbox.{cpu,memory,storage}, never resources', async () => {
			const spy = vi.spyOn(store, 'update');
			const alice = projectService(store, 'alice');
			await alice.setSandboxResources('a-proj', { cpu: 8, memory: '16Gi', storage: '40Gi' });
			expect(spy).toHaveBeenCalledWith('a-proj', {
				sandbox: { cpu: 8, memory: '16Gi', storage: '40Gi' }
			});
			expect(JSON.stringify(spy.mock.calls)).not.toContain('resources');
			expect((await alice.get('a-proj'))?.sandboxSettings).toEqual({
				cpu: 8,
				memory: '16Gi',
				storage: '40Gi'
			});
		});
		it('removes cleared cpu and memory and leaves storage alone when omitted', async () => {
			const alice = projectService(store, 'alice');
			await alice.setSandboxResources('a-proj', { cpu: 8, memory: '16Gi', storage: '40Gi' });
			await alice.setSandboxResources('a-proj', { cpu: null, memory: null });
			expect((await alice.get('a-proj'))?.sandboxSettings).toEqual({
				cpu: null,
				memory: null,
				storage: '40Gi'
			});
		});
		it('rejects non-owners and missing projects without patching', async () => {
			const spy = vi.spyOn(store, 'update');
			const alice = projectService(store, 'alice');
			const input = { cpu: 4, memory: null };
			await expect(alice.setSandboxResources('bobs', input)).rejects.toMatchObject({
				kind: 'not-found'
			});
			await expect(alice.setSandboxResources('ghost', input)).rejects.toMatchObject({
				kind: 'not-found'
			});
			expect(spy).not.toHaveBeenCalled();
		});
		it('rejects a storage shrink against the stored spec, defaulting to 20Gi', async () => {
			const spy = vi.spyOn(store, 'update');
			const alice = projectService(store, 'alice');
			await expect(
				alice.setSandboxResources('a-proj', { cpu: null, memory: null, storage: '10Gi' })
			).rejects.toMatchObject({ kind: 'invalid' });
			await alice.setSandboxResources('a-proj', { cpu: null, memory: null, storage: '40Gi' });
			spy.mockClear();
			await expect(
				alice.setSandboxResources('a-proj', { cpu: null, memory: null, storage: '30Gi' })
			).rejects.toMatchObject({ kind: 'invalid' });
			expect(spy).not.toHaveBeenCalled();
		});
	});
	it('deletes own projects', async () => {
		await projectService(store, 'alice').delete('a-proj');
		expect(store.items.has('a-proj')).toBe(false);
	});
});
