<script lang="ts">
	import { untrack } from 'svelte';
	import Button from './form/Button.svelte';
	import TextField from './form/TextField.svelte';
	export interface RepoRow {
		url: string;
		defaultBranch: string;
	}

	let {
		initial = [{ url: '', defaultBranch: 'main' }],
		issues = () => [],
		field = (i, f) => ({ name: `repositories[${i}].${f}` })
	}: {
		/** Starting rows; the form reads the inputs by name, so state lives here. */
		initial?: RepoRow[];
		/** Validation messages for a row's field. */
		issues?: (index: number, field: keyof RepoRow) => string[];
		/**
		 * Input attributes (name etc.) for a row's field. Inside a remote form pass
		 * `(i, f) => fields.repositories[i][f].as(f === 'url' ? 'url' : 'text')`, so names carry
		 * the form id SvelteKit needs to bind them.
		 */
		field?: (index: number, field: keyof RepoRow) => { name: string; [attr: string]: unknown };
	} = $props();
	const start = untrack(() => initial.map((r) => ({ ...r })));
	let rows = $state<RepoRow[]>(start.map((r) => ({ ...r })));

	/** A form reset restores the starting rows. */
	function resettable(node: HTMLElement) {
		const form = node.closest('form');
		const reset = () => (rows = start.map((r) => ({ ...r })));
		form?.addEventListener('reset', reset);
		return () => form?.removeEventListener('reset', reset);
	}
</script>

<fieldset class="repo-list" {@attach resettable}>
	<legend>Repositories</legend>
	{#each rows as row, i (i)}
		<div class="repo-row">
			<TextField
				label="URL"
				{...field(i, 'url')}
				placeholder="https://github.com/owner/repo"
				bind:value={row.url}
				issues={issues(i, 'url')}
			/>
			<TextField
				label="Default branch"
				{...field(i, 'defaultBranch')}
				bind:value={row.defaultBranch}
				issues={issues(i, 'defaultBranch')}
			/>
			<Button
				onclick={() => (rows = rows.filter((_, j) => j !== i))}
				aria-label="Remove repository {i + 1}">Remove</Button
			>
		</div>
	{/each}
	<Button onclick={() => (rows = [...rows, { url: '', defaultBranch: 'main' }])}
		>Add repository</Button
	>
</fieldset>
