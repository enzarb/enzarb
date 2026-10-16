<script lang="ts">
	import type { Subject } from '#lib/tasks.ts';

	/** The GitHub objects linked to a task, as links (`github:o/r#12` shows as `o/r#12`). */
	let { subjects }: { subjects: Subject[] } = $props();

	const label = (s: Subject) => s.title || s.key.replace(/^[a-z]+:/, '');
</script>

{#if subjects.length}
	<ul class="ez-task-subjects" aria-label="Subjects">
		{#each subjects as s (s.key)}
			<li>
				<span class="ez-muted">{s.kind.replace('_', ' ')}</span>
				{#if s.url}
					<a href={s.url} target="_blank" rel="noopener noreferrer">{label(s)}</a>
				{:else}
					<code>{label(s)}</code>
				{/if}
			</li>
		{/each}
	</ul>
{/if}
