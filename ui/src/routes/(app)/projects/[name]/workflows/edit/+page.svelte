<script lang="ts">
	import { page } from '$app/state';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import WorkflowEditor from '#lib/components/workflow/WorkflowEditor.svelte';
	import { EXAMPLE_ISSUE } from '#lib/workflow/example.ts';
	import { proposeChange } from '#lib/workflow/propose.ts';
	import {
		commitWorkflowProposal,
		openWorkflowProposal,
		startWorkflowProposal,
		workflowFile
	} from '#lib/workflows.remote.ts';

	const name = $derived(page.params.name!);
	const repo = $derived(page.url.searchParams.get('repo') ?? '');
	const path = $derived(page.url.searchParams.get('path'));
	const file = $derived(path ? workflowFile({ name, repo, path }) : null);
	const back = $derived(`/projects/${encodeURIComponent(name)}/workflows`);

	async function loadFile(p: string) {
		return (await workflowFile({ name, repo, path: p }))?.text ?? null;
	}
</script>

<svelte:head><title>Edit workflow · {name} · enzarb</title></svelte:head>

<p><a href={back}>← Workflows</a> · <span class="ez-muted">{repo}</span></p>

{#snippet editor(source: string, p: string | null)}
	<WorkflowEditor
		{source}
		path={p}
		{loadFile}
		propose={(files, workflow, step) =>
			proposeChange(
				{
					start: (a) => startWorkflowProposal({ name, repo, ...a }),
					commit: (a) => commitWorkflowProposal({ name, repo, ...a }),
					open: (a) => openWorkflowProposal({ name, repo, ...a })
				},
				files,
				workflow,
				step
			)}
	/>
{/snippet}

{#if !path}
	{@render editor(EXAMPLE_ISSUE, null)}
{:else if file?.error && !file.current}
	<Notice tone="error" title="Could not load {path}">Something went wrong while reading it.</Notice>
{:else if file?.current === undefined}
	<Skeleton label="Loading the definition" rows={4} />
{:else if file.current === null}
	<Notice tone="warning">{path} does not exist on the default branch.</Notice>
{:else}
	{#key path}{@render editor(file.current.text, path)}{/key}
{/if}
