<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { isHttpError } from '@sveltejs/kit';
	import Button from '#lib/components/form/Button.svelte';
	import ConditionChecklist from '#lib/components/ConditionChecklist.svelte';
	import CredentialStatus from '#lib/components/CredentialStatus.svelte';
	import ConfirmDialog from '#lib/components/ui/ConfirmDialog.svelte';
	import KeyValueList from '#lib/components/ui/KeyValueList.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import Tabs from '#lib/components/ui/Tabs.svelte';
	import SandboxResourcesForm from '#lib/components/SandboxResourcesForm.svelte';
	import Terminal from '#lib/components/Terminal.svelte';
	import * as v from 'valibot';
	import MaxRunsInput from '#lib/components/MaxRunsInput.svelte';
	import MaxRunsNotice from '#lib/components/MaxRunsNotice.svelte';
	import { isTerminalFailure } from '#lib/conditions.ts';
	import {
		deleteProject,
		getProject,
		setMaxRuns,
		setSandboxResources
	} from '#lib/projects.remote.ts';
	import { DEFAULT_MAX_RUNS, MaxRunsSchema } from '#lib/projects.ts';
	import { getProjectSettings } from '#lib/terminal.remote.ts';
	import PendingChangeBanner from '#lib/components/PendingChangeBanner.svelte';
	import { watchWorkItems } from '#lib/live.ts';
	import { countPossiblyRunning } from '#lib/work-items.ts';
	import { listWorkItems } from '#lib/work.remote.ts';

	const TABS = [
		{ value: 'overview', label: 'Overview' },
		{ value: 'terminal', label: 'Terminal' },
		{ value: 'settings', label: 'Settings' }
	];

	// The selected tab lives in `?tab=`; anything unknown is Overview.
	const tab = $derived.by(() => {
		const t = page.url.searchParams.get('tab');
		return TABS.some((x) => x.value === t) ? t! : 'overview';
	});

	function selectTab(value: string) {
		if (value === tab) return;
		const url = new URL(page.url.href);
		if (value === 'overview') url.searchParams.delete('tab');
		else url.searchParams.set('tab', value);
		void goto(url, { replace: true, reset: false });
	}

	const POLL_MS = 2000;
	const project = $derived(getProject(page.params.name!));
	const settings = $derived(getProjectSettings(page.params.name!));

	// True while a saved sandbox-resources change is still being applied.
	let trackingResources = $state(false);

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
			(project.current?.ready && !project.current.pendingChange && !trackingResources)
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

	const notFound = $derived(isHttpError(project.error, 404));

	// The sandbox's own answer, once it arrives (overrides the cached flag).
	let configured = $state<boolean | undefined>();
	// A different project has its own answer.
	$effect(() => {
		void page.params.name;
		configured = undefined;
	});

	let confirming = $state(false);
	let deleteError = $state('');

	// Unsaved edit of the concurrency setting; undefined while the user has not touched it.
	let draft = $state<number | null | '' | undefined>();
	let saving = $state(false);
	let saveError = $state('');
	$effect(() => {
		void page.params.name;
		draft = undefined;
		saveError = '';
	});

	async function saveMaxRuns(value: number) {
		saving = true;
		saveError = '';
		try {
			// The default is stored as "unset" so it follows the sandbox default.
			await setMaxRuns({
				name: page.params.name!,
				maxRuns: value === DEFAULT_MAX_RUNS ? null : value
			});
			draft = undefined;
		} catch {
			saveError = 'Could not save the setting.';
		} finally {
			saving = false;
		}
	}

	async function remove() {
		deleteError = '';
		try {
			await deleteProject(page.params.name!);
			await goto('/projects');
		} catch {
			deleteError = 'Could not delete the project.';
		}
	}
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
		<PageHeader title={p.displayName}>
			{#snippet meta()}
				<code>{p.name}</code>
				{#if p.sandboxName}<span class="ez-muted">sandbox <code>{p.sandboxName}</code></span>{/if}
			{/snippet}
		</PageHeader>
		{#if deleteError}<Notice tone="error">{deleteError}</Notice>{/if}

		<PendingChangeBanner change={p.pendingChange} {runningJobs} />

		<Tabs tabs={TABS} label="Project sections" bind:value={() => tab, selectTab}>
			{#snippet panel(current: string)}
				{#if current === 'overview'}
					<div class="ez-stack">
						{#if !p.ready || isTerminalFailure(p.conditions)}
							<Section title="Boot progress">
								<ConditionChecklist conditions={p.conditions} />
							</Section>
						{/if}

						<Section title="Repositories">
							{#each p.repositories as r (r.url)}
								<p><a href={r.url}>{r.url}</a> <span class="ez-muted">({r.defaultBranch})</span></p>
							{:else}
								<p class="ez-muted">No repositories.</p>
							{/each}
						</Section>

						<Section title="Triggers">
							<KeyValueList
								items={[
									{
										term: 'Issues',
										value: p.triggers.issues ? 'On' : 'Off',
										tone: p.triggers.issues ? 'success' : 'neutral'
									},
									{
										term: 'Change requests',
										value: p.triggers.changeRequests ? 'On' : 'Off',
										tone: p.triggers.changeRequests ? 'success' : 'neutral'
									},
									{
										term: 'Mentions',
										value: p.triggers.mentions ? 'On' : 'Off',
										tone: p.triggers.mentions ? 'success' : 'neutral'
									}
								]}
							/>
						</Section>
					</div>
				{:else if current === 'terminal'}
					{#if p.ready}
						<div class="ez-project-terminal">
							<Notice tone="info">
								The terminal is for emergencies. Prefer the automated workflows where you can.
							</Notice>
							<Terminal project={p.name} fill />
						</div>
					{:else}
						<Notice tone="info"
							>Sandbox is still booting. The terminal is available once it is ready.</Notice
						>
					{/if}
				{:else if current === 'settings'}
					{@const saved = p.maxRuns ?? DEFAULT_MAX_RUNS}
					{@const value = draft === undefined ? saved : draft}
					{@const valid = v.safeParse(MaxRunsSchema, value).success}
					<div class="ez-stack">
						<Section title="Concurrency">
							<div class="ez-settings">
								<MaxRunsInput bind:value={() => value, (x) => (draft = x)} />
								<Button
									variant="primary"
									disabled={!valid || value === saved || saving}
									onclick={() => saveMaxRuns(value as number)}
								>
									{saving ? 'Saving…' : 'Save'}
								</Button>
							</div>
							<MaxRunsNotice />
							{#if saveError}<Notice tone="error">{saveError}</Notice>{/if}
						</Section>

						<Section title="Sandbox resources">
							<SandboxResourcesForm
								project={p}
								bind:tracking={trackingResources}
								save={async (input) => {
									await setSandboxResources({ name: p.name, ...input });
								}}
							/>
						</Section>

						<Section title="Sandbox">
							<CredentialStatus
								project={p.name}
								cached={settings.current?.claudeTokenConfigured ?? false}
								onchecked={(c) => (configured = c)}
							/>
							{#if settings.current?.claudeTokenConfigured && settings.current.claudeConfiguredBy}
								<p class="ez-muted">Configured by {settings.current.claudeConfiguredBy}</p>
							{/if}
							<div class="ez-cluster">
								<LinkButton href="/projects/{p.name}/claude">
									{(configured ?? settings.current?.claudeTokenConfigured) ? 'Redo' : 'Start'} Claude
									setup
								</LinkButton>
							</div>
						</Section>

						<Section title="Danger zone">
							<p class="ez-muted">Deleting the project removes the sandbox and everything in it.</p>
							<div>
								<Button variant="danger" onclick={() => (confirming = true)}>Delete project</Button>
							</div>
						</Section>
					</div>
				{/if}
			{/snippet}
		</Tabs>

		<ConfirmDialog
			bind:open={confirming}
			title="Delete {p.displayName}?"
			description="The sandbox and everything in it are removed. This cannot be undone."
			confirmLabel="Confirm delete"
			onconfirm={remove}
		/>
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
