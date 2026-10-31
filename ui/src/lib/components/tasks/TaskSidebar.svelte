<script lang="ts">
	import { subjectLabel, type RepoContext } from '#lib/github-links.ts';
	import { KIND_NAMES, shortStatus, type Task, type TaskKind } from '#lib/tasks.ts';

	/** The task list, grouped by kind. Each entry selects its task through the URL. */
	let {
		groups,
		selected,
		href,
		showProject = false,
		context
	}: {
		groups: { kind: TaskKind; label: string; tasks: Task[] }[];
		selected: string | null;
		/** The URL that selects a task key. */
		href: (key: string) => string;
		showProject?: boolean;
		context?: RepoContext;
	} = $props();
</script>

<nav class="ez-tasks__sidebar" aria-label="Tasks">
	{#each groups as g (g.kind)}
		<h2 class="ez-tasks__group">{g.label} <span class="ez-muted">{g.tasks.length}</span></h2>
		<ul class="ez-tasks__entries" data-testid="tasks-{g.kind}">
			{#each g.tasks as t (t.key)}
				<li aria-label="{g.label}: {t.title}">
					<a
						class="ez-tasks__entry"
						href={href(t.key)}
						aria-current={t.key === selected ? 'true' : undefined}
					>
						<strong class="ez-tasks__title">{t.title}</strong>
						<span class="ez-muted">
							{KIND_NAMES[t.kind]}
							{#if t.number}{subjectLabel(t.repo, t.number, context)}{/if}
							{#if showProject}· {t.project}{/if}
							· {shortStatus(t)}
						</span>
					</a>
				</li>
			{/each}
		</ul>
	{/each}
</nav>
