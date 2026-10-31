<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { RepoContext } from '#lib/github-links.ts';
	import type { RunDetail } from '#lib/runs.ts';
	import {
		defaultAction,
		KIND_NAMES,
		shortStatus,
		type DefaultActionId,
		type Task
	} from '#lib/tasks.ts';
	import Button from '../form/Button.svelte';
	import RunLink from '../RunLink.svelte';
	import RunNodes from '../RunNodes.svelte';
	import SubjectLink from '../SubjectLink.svelte';
	import Badge from '../ui/Badge.svelte';
	import LinkButton from '../ui/LinkButton.svelte';
	import Section from '../ui/Section.svelte';
	import Skeleton from '../ui/Skeleton.svelte';
	import TaskChatBar from './TaskChatBar.svelte';

	/**
	 * One task: title and links, the default action as the primary button, secondary actions, the
	 * content of its kind, and the chat bar. Later changes add sections through `sections` and
	 * replace the chat bar through `chat`.
	 */
	let {
		task,
		run,
		context,
		busy = false,
		act,
		sections,
		chat
	}: {
		task: Task;
		/** The run behind a run task: `undefined` while loading, `null` when it cannot be read. */
		run?: RunDetail | null;
		context?: RepoContext;
		busy?: boolean;
		act: (id: DefaultActionId) => void;
		/** Extra sections under the built-in content (plan approval, review). */
		sections?: Snippet;
		/** Replaces the chat bar. */
		chat?: Snippet;
	} = $props();

	const primary = $derived(defaultAction(task));
</script>

<article class="ez-tasks__detail" aria-label="Task: {task.title}">
	<div class="ez-tasks__scroll">
		<header class="ez-tasks__head">
			<h2>{task.title}</h2>
			<p class="ez-tasks__links">
				<Badge tone="neutral" hollow>{KIND_NAMES[task.kind]}</Badge>
				<span class="ez-muted">{shortStatus(task)}</span>
				{#if task.number}
					<SubjectLink repo={task.repo} number={task.number} url={task.githubUrl} {context} />
				{/if}
				{#if (task.kind === 'wait' || task.kind === 'failed') && task.runUrl}
					<RunLink workflow={task.workflow} id={task.runId} href={task.runUrl} />
				{:else if task.kind === 'review' && task.runUrl}
					<a class="ez-run-link" href={task.runUrl}>Run that opened it</a>
				{/if}
			</p>
			<div class="ez-tasks__actions">
				{#if primary.id === 'diff'}
					<LinkButton
						variant="primary"
						href="{task.githubUrl ?? `https://github.com/${task.repo}/pull/${task.number}`}/files"
						target="_blank"
						rel="noopener noreferrer">{primary.label}</LinkButton
					>
				{:else}
					<Button variant="primary" loading={busy} onclick={() => act(primary.id)}
						>{primary.label}</Button
					>
				{/if}
				{#if (task.kind === 'wait' || task.kind === 'failed') && task.runUrl}
					<LinkButton href={task.runUrl}>Open run page</LinkButton>
				{/if}
			</div>
		</header>

		{#if task.kind === 'triaged'}
			<Section title="Issue">
				{#if task.body}
					<pre class="ez-tasks__body">{task.body}</pre>
				{:else}
					<p class="ez-muted">The issue has no description.</p>
				{/if}
			</Section>
		{:else if task.kind === 'review'}
			<Section title="Pull request">
				<p><code>{task.branch}</code> · {task.draft ? 'draft' : 'ready for review'}</p>
				<p>
					<SubjectLink repo={task.repo} number={task.number} url={task.githubUrl} {context} />
					— {task.title}
				</p>
			</Section>
		{:else if run === undefined}
			<Skeleton label="Loading run" rows={2} />
		{:else if run === null}
			<p class="ez-muted">The run could not be read. enzarb-tracks may be unreachable.</p>
		{:else}
			{#if task.kind === 'wait' && task.waitingFor}
				<p class="ez-muted">
					Waiting for {task.waitingFor}{task.until
						? `, until ${new Date(task.until).toLocaleString()}`
						: ''}
				</p>
			{/if}
			{#if run.reason}<p class="ez-noderuns__error" role="alert">{run.reason}</p>{/if}
			<RunNodes {run} />
		{/if}
		{@render sections?.()}
	</div>
	<div class="ez-tasks__chat">
		{#if chat}{@render chat()}{:else}<TaskChatBar />{/if}
	</div>
</article>
