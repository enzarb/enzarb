<script lang="ts">
	import {
		getProject,
		getOrgTierInfo,
		resizeStorage,
		removeProject,
		setProjectSuspendedCommand,
		getProjects,
		getDeletedProjects
	} from '$lib/remote/projects.remote';
	import { getProjectSecrets, setProjectSecret, deleteProjectSecret } from '$lib/remote/settings.remote';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { confirm } from '$lib/confirm';
	import { toErrorMessage } from '$lib/errors';
	import { getProjectColor, saveProjectColor, PROJECT_COLOR_PALETTE } from '$lib/tiling/layout';

	// Per-project tab color used across the tiling workspace. Stored client-side
	// (localStorage), so it's only read once mounted in the browser.
	let projectColor = $state('#6c6cff');
	onMount(() => {
		projectColor = getProjectColor(page.params.namespace!, page.params.project!);
	});
	function setProjectColor(color: string) {
		projectColor = color;
		saveProjectColor(page.params.namespace!, page.params.project!, color);
	}

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
			resizeError = toErrorMessage(e, 'Failed to resize storage');
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
			deleteError = toErrorMessage(e, 'Failed to delete project');
			deleting = false;
		}
	}

	let suspending = $state(false);
	let suspendError = $state('');

	async function handleSuspendToggle(slug: string, displayName: string, currentlySuspended: boolean) {
		if (!currentlySuspended) {
			const ok = await confirm({
				title: `Suspend project "${displayName}"?`,
				message:
					'The workspace and every environment are scaled to zero — no compute runs and no usage is billed. Nothing is deleted; resume anytime to bring it all back exactly as it was.',
				confirmText: 'Suspend',
				danger: true
			});
			if (!ok) return;
		}
		suspending = true;
		suspendError = '';
		try {
			await setProjectSuspendedCommand({ slug, suspended: !currentlySuspended });
		} catch (e) {
			suspendError = toErrorMessage(e, 'Failed to update suspend state');
		} finally {
			suspending = false;
		}
	}

	const projectData = $derived(Promise.all([getProject(page.params.project), getOrgTierInfo()]));
	const projectSecrets = $derived(getProjectSecrets());

	// Project-level env secrets
	let newSecretKey = $state('');
	let newSecretValue = $state('');
	let secretError = $state('');
	let addingSecret = $state(false);

	async function addSecret() {
		if (!newSecretKey.trim()) return;
		addingSecret = true;
		secretError = '';
		try {
			await setProjectSecret({ key: newSecretKey.trim(), value: newSecretValue });
			await getProjectSecrets().refresh();
			newSecretKey = '';
			newSecretValue = '';
		} catch (e) {
			secretError = toErrorMessage(e, 'Failed to save secret');
		} finally {
			addingSecret = false;
		}
	}

	async function removeSecret(key: string) {
		try {
			await deleteProjectSecret({ key });
			await getProjectSecrets().refresh();
		} catch (e) {
			secretError = toErrorMessage(e, 'Failed to delete secret');
		}
	}
</script>

{#await projectData then [project, { limits }]}
	{@const currentGi = parseInt(project.spec.storage?.size ?? '0')}
	{@const gi = resizeGi ?? currentGi + 1}

	<div class="settings-page">
		<section class="card">
			<h3>Environment Variables</h3>
			<p class="muted">Project-level secrets injected as environment variables. They override user-level secrets with the same key.</p>
			{#await projectSecrets}
				<p class="muted">Loading…</p>
			{:then secrets}
				{#if secrets.length > 0}
					<ul class="secret-list">
						{#each secrets as s}
							<li>
								<code class="mono">{s.key}</code>
								<span class="secret-value">••••••••</span>
								<button class="btn-icon" onclick={() => removeSecret(s.key)} title="Delete">✕</button>
							</li>
						{/each}
					</ul>
				{/if}
			{/await}
			<div class="secret-add-row">
				<input
					class="input-key"
					type="text"
					placeholder="KEY"
					bind:value={newSecretKey}
					onkeydown={(e) => e.key === 'Enter' && addSecret()}
				/>
				<input
					class="input-value"
					type="password"
					placeholder="value"
					bind:value={newSecretValue}
					onkeydown={(e) => e.key === 'Enter' && addSecret()}
				/>
				<button class="btn btn-primary" disabled={addingSecret || !newSecretKey.trim()} onclick={addSecret}>
					{addingSecret ? 'Saving…' : 'Add'}
				</button>
			</div>
			{#if secretError}<p class="error-text">{secretError}</p>{/if}
		</section>

		<section class="card">
			<h3>Tab color</h3>
			<p class="muted">Colors this project's tabs and swatches in the tiling workspace so panes from different projects stay easy to tell apart.</p>
			<div class="color-row">
				<span class="color-preview" style="background: {projectColor}"></span>
				<div class="swatches">
					{#each PROJECT_COLOR_PALETTE as c}
						<button
							type="button"
							class="swatch"
							class:selected={projectColor.toLowerCase() === c.toLowerCase()}
							style="background: {c}"
							title={c}
							aria-label={c}
							onclick={() => setProjectColor(c)}
						></button>
					{/each}
				</div>
				<input
					class="color-hex"
					type="text"
					spellcheck="false"
					value={projectColor}
					onchange={(e) => setProjectColor(e.currentTarget.value)}
					placeholder="#6c6cff"
				/>
			</div>
		</section>

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

		<section class="card">
			<div class="suspend-row">
				<div>
					<h3>{project.spec.suspended ? 'Suspended' : 'Suspend project'}</h3>
					<p class="muted">
						{#if project.spec.suspended}
							Workspace and environments are scaled to zero. Nothing was deleted — resume to bring it all back.
						{:else}
							Temporarily scale the workspace and every environment to zero. No data is touched and this can be reversed at any time — unlike delete, there's no retention window or purge.
						{/if}
					</p>
					{#if suspendError}<p class="error-text">{suspendError}</p>{/if}
				</div>
				<button
					class="btn {project.spec.suspended ? 'btn-primary' : 'btn-danger'}"
					disabled={suspending}
					onclick={() => handleSuspendToggle(project.metadata.name, project.spec.displayName, !!project.spec.suspended)}
				>
					{#if suspending}
						{project.spec.suspended ? 'Resuming…' : 'Suspending…'}
					{:else}
						{project.spec.suspended ? 'Resume project' : 'Suspend project'}
					{/if}
				</button>
			</div>
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
	.suspend-row { display: flex; justify-content: space-between; align-items: center; gap: 1rem; }
	.danger { display: flex; justify-content: space-between; align-items: center; gap: 1rem; border-color: var(--color-danger, #c0392b); }
	.danger h3 { margin-bottom: 0.25rem; }
	.danger .muted { margin-bottom: 0; }
	.btn-danger { background: var(--color-danger, #c0392b); color: #fff; border: none; flex-shrink: 0; }
	.btn-danger:disabled { opacity: 0.6; cursor: default; }
	.secret-list { list-style: none; margin: 0 0 0.75rem; padding: 0; display: flex; flex-direction: column; gap: 0.375rem; }
	.secret-list li { display: flex; align-items: center; gap: 0.5rem; font-size: 13px; }
	.secret-value { color: var(--color-text-muted); flex: 1; }
	.btn-icon { background: none; border: none; color: var(--color-text-muted); cursor: pointer; padding: 0 0.25rem; line-height: 1; }
	.btn-icon:hover { color: var(--color-danger, #c0392b); }
	.secret-add-row { display: flex; gap: 0.5rem; align-items: center; }
	.input-key { flex: 0 0 160px; box-sizing: border-box; padding: 0.375rem 0.5rem; border: 1px solid var(--color-border); border-radius: var(--radius); background: var(--color-surface-2); color: var(--color-text); font-size: 13px; line-height: 1.4; font-family: var(--font-mono); }
	.input-value { flex: 1 1 0; min-width: 0; box-sizing: border-box; padding: 0.375rem 0.5rem; border: 1px solid var(--color-border); border-radius: var(--radius); background: var(--color-surface-2); color: var(--color-text); font-size: 13px; line-height: 1.4; }
	.color-row { display: flex; align-items: center; gap: 0.75rem; flex-wrap: wrap; }
	.color-preview { width: 24px; height: 24px; border-radius: 50%; border: 1px solid var(--color-border); flex-shrink: 0; }
	.swatches { display: flex; gap: 0.375rem; flex-wrap: wrap; }
	.swatch { width: 22px; height: 22px; border-radius: 50%; border: 2px solid transparent; padding: 0; cursor: pointer; }
	.swatch.selected { border-color: var(--color-text); }
	.color-hex { width: 100px; padding: 0.375rem 0.5rem; border: 1px solid var(--color-border); border-radius: var(--radius); background: var(--color-surface-2); color: var(--color-text); font-size: 12px; font-family: var(--font-mono); }
</style>
