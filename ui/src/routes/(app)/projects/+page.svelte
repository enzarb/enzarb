<script lang="ts">
	import StatusBadge from '#lib/components/StatusBadge.svelte';
	import DataTable, { type Column } from '#lib/components/ui/DataTable.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import { overallState } from '#lib/conditions.ts';
	import type { ProjectView } from '#lib/project-types.ts';
	import { listProjects } from '#lib/projects.remote.ts';

	const projects = listProjects();

	const columns: Column<ProjectView>[] = [
		{ id: 'project', header: 'Project', cell: project },
		{ id: 'name', header: 'Name', cell: name },
		{ id: 'status', header: 'Status', cell: status }
	];
</script>

{#snippet project(p: ProjectView)}<a href="/projects/{p.name}">{p.displayName}</a>{/snippet}
{#snippet name(p: ProjectView)}<code>{p.name}</code>{/snippet}
{#snippet status(p: ProjectView)}<StatusBadge state={overallState(p.conditions)} />{/snippet}

<svelte:head><title>Projects · enzarb</title></svelte:head>
<PageHeader title="Projects" description="Sandboxes for your code repositories.">
	{#snippet actions()}
		<LinkButton variant="primary" href="/projects/new">New project</LinkButton>
	{/snippet}
</PageHeader>

<svelte:boundary>
	{#if projects.error}
		<Notice tone="error">Could not load projects.</Notice>
	{:else}
		<DataTable
			label="Projects"
			items={projects.current ?? []}
			{columns}
			rowKey={(p) => p.name}
			loading={projects.loading && !projects.current}
			loadingLabel="Loading projects"
			emptyTitle="No projects yet"
			emptyDescription="Create a project to give a repository its own sandbox."
		>
			{#snippet emptyActions()}
				<LinkButton variant="primary" href="/projects/new">New project</LinkButton>
			{/snippet}
		</DataTable>
	{/if}
	{#snippet failed()}<Notice tone="error">Could not load projects.</Notice>{/snippet}
</svelte:boundary>
