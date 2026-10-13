<script lang="ts">
	import type { Snippet } from 'svelte';
	import { page } from '$app/state';
	import { isHttpError } from '@sveltejs/kit';
	import Button from '#lib/components/form/Button.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import PendingChangeBanner from '#lib/components/PendingChangeBanner.svelte';
	import ProjectHeader from '#lib/components/ProjectHeader.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getProject } from '#lib/projects.remote.ts';
	import { setProjectContext } from '#lib/project-context.svelte.ts';
	import { watchWorkItems } from '#lib/live.ts';
	import { countPossiblyRunning } from '#lib/work-items.ts';
	import { listWorkItems } from '#lib/work.remote.ts';

	let { children }: { children: Snippet } = $props();

	const POLL_MS = 2000;
	const project = $derived(getProject(page.params.name!));

	// True while a saved sandbox-resources change is still being applied (set by the Settings tab).
	const shared = $state({ trackingResources: false });
	setProjectContext({
		get project() {
			return project.current!;
		},
		get trackingResources() {
			return shared.trackingResources;
		},
		set trackingResources(v) {
			shared.trackingResources = v;
		}
	});

	const workItems = $derived(listWorkItems(page.params.name!));
	// Approximation of the jobs the operator waits for: work items in an active state. The workflows
	// contract does not list which states hold a running pod, so only zero/non-zero is used and the
	// count is never shown. null until the list has loaded, so the banner does not claim the jobs are finished.
	const runningJobs = $derived(workItems.current ? countPossiblyRunning(workItems.current) : null);

	// Poll while the project is not Ready, has a change pending or a sandbox-resources change is in
	// flight (no timers when idle). A failed poll is ignored until the next tick; it must not replace
	// the page.
	$effect(() => {
		if (
			(project.error && !project.current) ||
			(project.current?.ready && !project.current.pendingChange && !shared.trackingResources)
		)
			return;
		const id = setInterval(() => {
			if (project.loading) return;
			project.refresh().catch(() => {});
		}, POLL_MS);
		return () => clearInterval(id);
	});

	// Refresh the job count promptly while a change is pending.
	$effect(() => {
		if (!project.current?.pendingChange) return;
		return watchWorkItems(() => void workItems.refresh());
	});

	// A different project starts with a clean slate.
	$effect(() => {
		void page.params.name;
		shared.trackingResources = false;
	});

	const notFound = $derived(isHttpError(project.error, 404));
</script>

<svelte:head><title>{page.params.name} · enzarb</title></svelte:head>

<svelte:boundary>
	{#if project.error && !project.current}
		{#if notFound}
			<PageHeader title="Project not found" />
			<div><LinkButton href="/projects">Back to projects</LinkButton></div>
		{:else}
			<Notice tone="error" title="Could not load the project">
				Something went wrong while loading this project.
				{#snippet actions()}
					<Button onclick={() => project.refresh().catch(() => {})}>Retry</Button>
				{/snippet}
			</Notice>
		{/if}
	{:else if project.current}
		{@const p = project.current}
		<div class="ez-project-view">
			<ProjectHeader
				displayName={p.displayName}
				name={p.name}
				sandboxName={p.sandboxName}
				pathname={page.url.pathname}
			/>
			<PendingChangeBanner change={p.pendingChange} {runningJobs} />
			<div class="ez-tabnav-panel">
				{@render children()}
			</div>
		</div>
	{:else}
		<Skeleton label="Loading project" rows={4} />
	{/if}
	{#snippet failed(e, reset)}
		{#if isHttpError(e, 404)}
			<PageHeader title="Project not found" />
			<div><LinkButton href="/projects">Back to projects</LinkButton></div>
		{:else}
			<Notice tone="error" title="Could not load the project">
				Something went wrong while loading this project.
				{#snippet actions()}
					<Button onclick={reset}>Retry</Button>
				{/snippet}
			</Notice>
		{/if}
	{/snippet}
</svelte:boundary>
