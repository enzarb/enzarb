<script lang="ts">
	import { groupTasks, type Task, type TaskList } from '#lib/tasks.ts';
	import Button from './form/Button.svelte';
	import Badge from './ui/Badge.svelte';
	import EmptyState from './ui/EmptyState.svelte';
	import Notice from './ui/Notice.svelte';
	import Section from './ui/Section.svelte';
	import Skeleton from './ui/Skeleton.svelte';

	/** Everything that waits on a human, grouped by kind; each task links to GitHub and its run. */
	let {
		list,
		showProject = false,
		failed = false,
		retry,
		implement,
		retryRun,
		stop
	}: {
		/** `undefined` while the first load is in flight. */
		list: TaskList | undefined;
		/** The top-level page spans Projects: show which one a task belongs to. */
		showProject?: boolean;
		failed?: boolean;
		retry: () => void;
		implement: (t: Task) => Promise<string>;
		retryRun: (t: Task) => Promise<string>;
		stop: (t: Task) => Promise<string>;
	} = $props();

	let busy = $state<string | null>(null);
	let message = $state('');

	async function act(t: Task, run: (t: Task) => Promise<string>) {
		busy = t.key;
		message = '';
		try {
			message = await run(t);
		} catch {
			message = 'That did not work. Try again.';
		} finally {
			busy = null;
		}
	}

	const groups = $derived(list ? groupTasks(list.tasks) : []);
	const where = (t: Task) =>
		`${showProject ? `${t.project} · ` : ''}${t.repo}${t.number ? `#${t.number}` : ''}`;
	const when = (iso: string) => new Date(iso).toLocaleString();
</script>

{#if failed}
	<Notice tone="error" title="Could not load tasks">
		Something went wrong while loading the tasks.
		{#snippet actions()}<Button onclick={retry}>Retry</Button>{/snippet}
	</Notice>
{:else if list === undefined}
	<Skeleton label="Loading tasks" rows={3} />
{:else}
	{#if message}<div role="status" class="ez-muted">{message}</div>{/if}
	{#each list.notes as note (note)}
		<Notice tone="warning" title="Some tasks may be missing">{note}</Notice>
	{/each}
	{#if groups.length === 0}
		<EmptyState
			title="Nothing is waiting for you"
			description="Runs parked at a wait, failed runs, pull requests from ez branches and triaged issues show up here."
		/>
	{/if}
	{#each groups as g (g.kind)}
		<Section title={g.label} description="{g.tasks.length} open">
			<ul class="ez-runs" data-testid="tasks-{g.kind}">
				{#each g.tasks as t (t.key)}
					<li class="ez-runs__row" aria-label="{g.label}: {t.title}">
						<strong>{t.title}</strong>
						<span class="ez-muted">{where(t)}</span>
						{#if t.kind === 'wait'}
							<Badge tone="neutral" hollow>waiting</Badge>
							{#if t.waitingFor}<span class="ez-muted">for {t.waitingFor}</span>{/if}
							{#if t.until}<span class="ez-muted">until {when(t.until)}</span>{/if}
						{:else if t.kind === 'failed'}
							<Badge tone="danger">failed</Badge>
							{#if t.reason}<span class="ez-muted">{t.reason}</span>{/if}
						{:else if t.kind === 'review'}
							<Badge tone="info">{t.draft ? 'draft' : 'open'}</Badge>
							<code>{t.branch}</code>
						{:else}
							<Badge tone="accent" plain>triaged</Badge>
						{/if}
						{#if t.githubUrl}
							<a href={t.githubUrl} target="_blank" rel="noopener noreferrer">GitHub</a>
						{/if}
						{#if t.runUrl}<a href={t.runUrl}>Run</a>{/if}
						{#if t.kind === 'wait'}
							<Button size="sm" loading={busy === t.key} onclick={() => act(t, stop)}>Stop</Button>
						{:else if t.kind === 'failed'}
							<Button size="sm" loading={busy === t.key} onclick={() => act(t, retryRun)}
								>Retry</Button
							>
						{:else if t.kind === 'triaged'}
							<Button
								size="sm"
								variant="primary"
								loading={busy === t.key}
								onclick={() => act(t, implement)}>Implement</Button
							>
						{/if}
					</li>
				{/each}
			</ul>
		</Section>
	{/each}
{/if}
