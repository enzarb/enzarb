<script lang="ts">
	import {
		getProject,
		getOrgTierInfo,
		resizeStorage,
		removeProject,
		getProjects,
		getDeletedProjects
	} from '$lib/remote/projects.remote';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { confirm } from '$lib/confirm';

	let resizing = $state(false);
	let resizeError = $state('');
	let resizeGi = $state<number | null>(null);

	async function handleResize(slug: string, currentGi: number, newGi: number, maxGi: number) {
		if (newGi <= currentGi) {
			resizeError = `New size must be larger than current ${currentGi}Gi`;
			return;
		}
		if (newGi > maxGi) {
			resizeError = `Cannot exceed tier limit of ${maxGi}Gi`;
			return;
		}
		resizing = true;
		resizeError = '';
		try {
			await resizeStorage({ slug, storageGi: newGi });
			await getProject().refresh();
		} catch (e) {
			resizeError = e instanceof Error ? e.message : 'Failed to resize storage';
		} finally {
			resizing = false;
		}
	}

	let deleting = $state(false);
	let deleteError = $state('');

	async function handleDelete(slug: string, displayName: string) {
		const ok = await confirm({
			title: `Delete project "${displayName}"?`,
			message:
				'The workspace stops immediately. It stays recoverable for the retention window, after which all data is permanently purged.',
			requireText: slug,
			confirmText: 'Delete',
			danger: true
		});
		if (!ok) return;
		deleting = true;
		deleteError = '';
		try {
			await removeProject({ slug });
			await Promise.all([getProjects().refresh(), getDeletedProjects().refresh()]);
			await goto(`/${page.params.namespace}/projects`);
		} catch (e) {
			deleteError = e instanceof Error ? e.message : 'Failed to delete project';
			deleting = false;
		}
	}
</script>

{#await Promise.all([getProject(), getOrgTierInfo()]) then [project, { limits }]}
	{@const currentGi = parseInt(project.spec.storage?.size ?? '0')}
	{@const gi = resizeGi ?? currentGi + 1}

	<div class="settings-page">
		<section class="card">
			<h3>Storage</h3>
			<p class="muted">Current allocation: <code class="mono">{project.spec.storage?.size ?? '–'}</code>. PVCs can only be enlarged — resizing applies on the next reconcile.</p>
			<div class="resize-row">
				<input
					type="number"
					min={currentGi + 1}
					max={limits.maxPvcGi}
					value={gi}
					oninput={(e) => (resizeGi = parseInt(e.currentTarget.value))}
					disabled={resizing}
				/>
				<span class="unit">GiB</span>
				<button
					class="btn btn-primary"
					disabled={resizing || gi <= currentGi || gi > limits.maxPvcGi}
					onclick={() => handleResize(project.metadata.name, currentGi, gi, limits.maxPvcGi)}
				>
					{resizing ? 'Resizing…' : 'Resize'}
				</button>
			</div>
			<p class="hint">Tier limit: {limits.maxPvcGi}Gi</p>
			{#if resizeError}<p class="error-text">{resizeError}</p>{/if}
		</section>

		<section class="card danger">
			<div>
				<h3>Delete project</h3>
				<p class="muted">Stops the workspace and schedules it for deletion. Recoverable during the retention window, then permanently purged along with its repository and data.</p>
				{#if deleteError}<p class="error-text">{deleteError}</p>{/if}
			</div>
			<button
				class="btn btn-danger"
				disabled={deleting}
				onclick={() => handleDelete(project.metadata.name, project.spec.displayName)}
			>
				{deleting ? 'Deleting…' : 'Delete project'}
			</button>
		</section>
	</div>
{:catch}
	<p class="muted">Could not load project settings.</p>
{/await}

<style>
	.settings-page { display: flex; flex-direction: column; gap: 1.5rem; max-width: 640px; }
	h3 { font-size: 14px; margin: 0 0 0.5rem; }
	.muted { color: var(--color-text-muted); font-size: 13px; margin: 0 0 0.75rem; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.resize-row { display: flex; align-items: center; gap: 0.5rem; }
	.resize-row input { width: 80px; padding: 0.375rem 0.5rem; border: 1px solid var(--color-border); border-radius: var(--radius); background: var(--color-surface-2); color: var(--color-text); font-size: 13px; }
	.unit { font-size: 13px; color: var(--color-text-muted); }
	.hint { font-size: 12px; color: var(--color-text-muted); margin: 0.4rem 0 0; }
	.error-text { color: var(--color-danger, #c0392b); font-size: 13px; margin-top: 0.5rem; }
	.danger { display: flex; justify-content: space-between; align-items: center; gap: 1rem; border-color: var(--color-danger, #c0392b); }
	.danger h3 { margin-bottom: 0.25rem; }
	.danger .muted { margin-bottom: 0; }
	.btn-danger { background: var(--color-danger, #c0392b); color: #fff; border: none; flex-shrink: 0; }
	.btn-danger:disabled { opacity: 0.6; cursor: default; }
</style>
