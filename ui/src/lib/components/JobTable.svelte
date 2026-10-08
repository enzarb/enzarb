<script lang="ts">
	import type { JobView } from '#lib/work-items.ts';

	let { jobs }: { jobs: JobView[] } = $props();
</script>

{#if jobs.length}
	<table class="table">
		<thead>
			<tr><th>Job</th><th>Status</th><th>Attempts</th><th>Output</th></tr>
		</thead>
		<tbody>
			{#each jobs as j (j.id)}
				<tr>
					<td><code>{j.kind}</code></td>
					<td><span class="badge {j.status}">{j.status}</span></td>
					<td>{j.attempt}</td>
					<td>
						{#if j.outputHtml}
							<!-- Sanitized on the server (sanitize-html allow-list) before it is sent. -->
							<!-- eslint-disable-next-line svelte/no-at-html-tags -->
							<div class="markdown">{@html j.outputHtml}</div>
						{:else}
							<span class="muted">—</span>
						{/if}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
{:else}
	<p class="muted">No jobs yet.</p>
{/if}
