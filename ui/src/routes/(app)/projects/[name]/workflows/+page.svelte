<script lang="ts">
	import { page } from '$app/state';
	import Button from '#lib/components/form/Button.svelte';
	import Select from '#lib/components/form/Select.svelte';
	import type { DryRunApi } from '#lib/components/workflows/DryRunPanel.svelte';
	import WorkflowsDesigner, {
		type WorkflowsApi
	} from '#lib/components/workflows/WorkflowsDesigner.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import {
		checkWorkflowsText,
		dryRunWorkflows,
		getWorkflows,
		listRepoEvents,
		renderWorkflowsFile,
		saveWorkflowsFile
	} from '#lib/workflows.remote.ts';

	const name = $derived(page.params.name!);
	let repo = $state<string | undefined>(undefined);
	const workflows = $derived(getWorkflows({ name, repo }));
	let committed = $state<{ unchanged: boolean; commitUrl: string | null } | null>(null);

	type Loaded = { repo: string; sha: string | null; modesToml: string | null };
	const api = (r: Loaded): WorkflowsApi => ({
		render: (file) => renderWorkflowsFile({ name, file, modesToml: r.modesToml }),
		check: (text) => checkWorkflowsText({ name, text, modesToml: r.modesToml }),
		save: (input) => saveWorkflowsFile({ name, repo: r.repo, baseSha: r.sha, ...input }),
		reload: () => {
			committed = null;
			void workflows.refresh();
		}
	});
	const dryRun = (r: Loaded): DryRunApi => ({
		events: (before) => listRepoEvents({ name, repo: r.repo, before }),
		run: (input) => dryRunWorkflows({ name, repo: r.repo, modesToml: r.modesToml, ...input })
	});
</script>

<svelte:head><title>Workflows · {page.params.name} · enzarb</title></svelte:head>

{#if workflows.error && !workflows.current}
	<Notice tone="error" title="Could not load the workflows">
		Something went wrong while reading the workflows.
		{#snippet actions()}<Button onclick={() => void workflows.refresh()}>Retry</Button>{/snippet}
	</Notice>
{:else if !workflows.current}
	<Skeleton label="Loading the workflows" rows={4} />
{:else if !workflows.current.ok}
	<Notice
		tone={workflows.current.code === 'unavailable' ? 'info' : 'error'}
		title="The workflow designer is not available"
	>
		{workflows.current.message}
		{#snippet actions()}
			{#if workflows.current?.ok === false && workflows.current.code !== 'unavailable'}
				<Button onclick={() => void workflows.refresh()}>Retry</Button>
			{/if}
		{/snippet}
	</Notice>
{:else}
	{@const r = workflows.current}
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
				{/if}
			</Notice>
		{/if}
		{#key `${r.repo}@${r.sha}`}
			<WorkflowsDesigner file={r} api={api(r)} dryRun={dryRun(r)} saved={(s) => (committed = s)} />
		{/key}
	</div>
{/if}
