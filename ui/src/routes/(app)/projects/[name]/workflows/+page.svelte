<script lang="ts">
	import { page } from '$app/state';
	import Select from '#lib/components/form/Select.svelte';
	import WorkflowViewer from '#lib/components/tasks/WorkflowViewer.svelte';
	import EmptyState from '#lib/components/ui/EmptyState.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getProjectContext } from '#lib/project-context.svelte.ts';
	import { normalizeRepoKey } from '#lib/projects.ts';
	import { getWorkflowView } from '#lib/tasks.remote.ts';

	const ctx = getProjectContext();
	const name = $derived(page.params.name!);
	const repos = $derived(
		ctx.project.repositories
			.map((r) => normalizeRepoKey(r.connector, r.url))
			.filter((k): k is string => !!k?.startsWith('github:'))
			.map((k) => k.slice('github:'.length))
	);
	let picked = $state<string | null>(null);
	const repo = $derived(picked && repos.includes(picked) ? picked : (repos[0] ?? null));
	const view = $derived(repo ? getWorkflowView({ project: name, repo }) : null);
</script>

<svelte:head><title>Workflows · {name} · enzarb</title></svelte:head>

{#if repos.length === 0}
	<EmptyState
		title="No GitHub repositories"
		description="Add a repository in Settings to see its workflows."
	/>
{:else}
	<div class="ez-stack">
		{#if repos.length > 1}
			<div class="ez-toolbar">
				<div class="ez-toolbar__item">
					<Select
						label="Repository"
						value={repo ?? ''}
						onchange={(v) => (picked = v)}
						options={repos.map((r) => ({ value: r, label: r }))}
					/>
				</div>
			</div>
		{/if}
		<p class="ez-muted">
			Read-only: the resolved bindings and workflows. Change them with a pull request to
			<code>.enzarb/</code> or the library.
		</p>
		{#if view?.error && !view.current}
			<Notice tone="error" title="Could not load workflows"
				>Something went wrong while resolving the workflows.</Notice
			>
		{:else if view && view.current === undefined}
			<Skeleton label="Loading workflows" rows={3} />
		{:else if view?.current === null}
			<EmptyState
				title="No resolved workflows"
				description="The engine has not resolved this repository yet."
			/>
		{:else if view?.current}
			<WorkflowViewer view={view.current} />
		{/if}
	</div>
{/if}
