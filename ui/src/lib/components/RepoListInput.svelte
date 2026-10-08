<script lang="ts">
	import { untrack } from 'svelte';
	export interface RepoRow {
		url: string;
		defaultBranch: string;
	}

	let {
		initial = [{ url: '', defaultBranch: 'main' }],
		issues = () => []
	}: {
		/** Starting rows; the form reads the inputs by name, so state lives here. */
		initial?: RepoRow[];
		/** Validation messages for a row's field. */
		issues?: (index: number, field: keyof RepoRow) => string[];
	} = $props();
	let rows = $state<RepoRow[]>(untrack(() => initial));
</script>

<fieldset class="repo-list">
	<legend>Repositories</legend>
	{#each rows as row, i (i)}
		<div class="repo-row">
			<label>
				<span class="muted">URL</span>
				<input
					name="repositories[{i}].url"
					type="url"
					placeholder="https://github.com/owner/repo"
					bind:value={row.url}
					aria-invalid={issues(i, 'url').length > 0}
				/>
			</label>
			<label>
				<span class="muted">Default branch</span>
				<input name="repositories[{i}].defaultBranch" bind:value={row.defaultBranch} />
			</label>
			<button
				type="button"
				onclick={() => (rows = rows.filter((_, j) => j !== i))}
				aria-label="Remove repository {i + 1}">Remove</button
			>
			{#each [...issues(i, 'url'), ...issues(i, 'defaultBranch')] as msg (msg)}
				<p class="field-error">{msg}</p>
			{/each}
		</div>
	{/each}
	<button type="button" onclick={() => (rows = [...rows, { url: '', defaultBranch: 'main' }])}
		>Add repository</button
	>
</fieldset>
