<script lang="ts">
	import { page } from '$app/state';
	import Button from '#lib/components/form/Button.svelte';
	import Select from '#lib/components/form/Select.svelte';
	import ModesDesigner, { type DesignerApi } from '#lib/components/modes/ModesDesigner.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import {
		getModes,
		parseModesText,
		renderModesText,
		saveModesFile,
		validateModes
	} from '#lib/modes.remote.ts';

	const name = $derived(page.params.name!);
	let repo = $state<string | undefined>(undefined);
	const modes = $derived(getModes({ name, repo }));
	let committed = $state<{ unchanged: boolean; commitUrl: string | null } | null>(null);

	const api = (r: { repo: string; sha: string | null }): DesignerApi => ({
		validate: (doc) => validateModes({ name, doc }),
		parse: (text) => parseModesText({ name, text }),
		render: (doc, old) => renderModesText({ name, doc, old }),
		save: (input) => saveModesFile({ name, repo: r.repo, baseSha: r.sha, ...input }),
		reload: () => {
			committed = null;
			void modes.refresh();
		}
	});
</script>

<svelte:head><title>Modes · {page.params.name} · enzarb</title></svelte:head>

{#if modes.error && !modes.current}
	<Notice tone="error" title="Could not load the modes">
		Something went wrong while reading the modes.
		{#snippet actions()}<Button onclick={() => void modes.refresh()}>Retry</Button>{/snippet}
	</Notice>
{:else if !modes.current}
	<Skeleton label="Loading the modes" rows={4} />
{:else if !modes.current.ok}
	<Notice
		tone={modes.current.code === 'unavailable' ? 'info' : 'error'}
		title="The modes designer is not available"
	>
		{modes.current.message}
		{#snippet actions()}
			{#if modes.current?.ok === false && modes.current.code !== 'unavailable'}
				<Button onclick={() => void modes.refresh()}>Retry</Button>
			{/if}
		{/snippet}
	</Notice>
{:else}
	{@const r = modes.current}
	<div class="ez-stack">
		{#if r.repos.length > 1}
			<div class="ez-max-select">
				<Select
					label="Repository"
					options={r.repos.map((k) => ({ value: k, label: k.replace(/^github:/, '') }))}
					bind:value={
						() => r.repo,
						(v) => {
							committed = null;
							repo = v;
						}
					}
				/>
			</div>
		{/if}
		{#if committed}
			<Notice tone="success">
				{#if committed.unchanged}
					Nothing changed, so nothing was committed.
				{:else}
					Committed to {r.branch}.
					{#if committed.commitUrl}<a href={committed.commitUrl} target="_blank" rel="noreferrer"
							>View the commit</a
						>{/if}
					Chats in a worktree that contains this commit use the new modes.
				{/if}
			</Notice>
		{/if}
		{#key `${r.repo}@${r.sha}`}
			<ModesDesigner file={r} api={api(r)} saved={(s) => (committed = s)} />
		{/key}
	</div>
{/if}
