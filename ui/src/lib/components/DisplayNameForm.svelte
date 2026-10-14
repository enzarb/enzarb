<script lang="ts">
	import type { ProjectView } from '#lib/project-types.ts';
	import { DISPLAY_NAME_MAX } from '#lib/projects.ts';
	import Button from './form/Button.svelte';
	import TextField from './form/TextField.svelte';
	import Notice from './ui/Notice.svelte';

	export type SaveOutcome = { ok: true } | { ok: false; message: string };

	/** The Project's display name; `metadata.name` (URLs, keys) never changes. Owner only. */
	let {
		project,
		save,
		canEdit = true
	}: {
		project: ProjectView;
		save: (displayName: string) => Promise<SaveOutcome>;
		canEdit?: boolean;
	} = $props();

	let editing = $state(false);
	let value = $state('');
	let saving = $state(false);
	let failure = $state('');

	const trimmed = $derived(value.trim());
	const valid = $derived(trimmed.length > 0 && trimmed.length <= DISPLAY_NAME_MAX);
	const dirty = $derived(trimmed !== project.displayName);

	function edit() {
		value = project.displayName;
		failure = '';
		editing = true;
	}
	function cancel() {
		value = project.displayName;
		failure = '';
		editing = false;
	}

	async function submit() {
		saving = true;
		failure = '';
		try {
			const r = await save(trimmed);
			if (r.ok) editing = false;
			else failure = r.message;
		} catch {
			failure = 'Could not save the display name.';
		} finally {
			saving = false;
		}
	}
</script>

<div class="ez-stack">
	{#if canEdit && editing}
		<TextField label="Display name" maxlength={DISPLAY_NAME_MAX} disabled={saving} bind:value />
		{#if failure}<Notice tone="error">{failure}</Notice>{/if}
		<div>
			<Button variant="primary" disabled={!valid || !dirty || saving} onclick={submit}>
				{saving ? 'Saving…' : 'Save display name'}
			</Button>
			<Button variant="ghost" disabled={saving} onclick={cancel}>Cancel</Button>
		</div>
	{:else}
		<p data-testid="display-name">{project.displayName}</p>
		{#if canEdit}
			<div><Button onclick={edit}>Edit display name</Button></div>
		{:else}
			<p class="ez-muted">Only the project owner can change these settings.</p>
		{/if}
	{/if}
</div>
