<script lang="ts">
	import { notInline, parsePatch, type PullFile } from '#lib/patch.ts';

	/** One changed file of a pull request: collapsible header, unified hunks with line numbers. */
	let { file, url, open = true }: { file: PullFile; url: string; open?: boolean } = $props();

	const skipped = $derived(notInline(file));
	const lines = $derived(skipped ? [] : parsePatch(file.patch ?? ''));
</script>

<details class="ez-diff" {open}>
	<summary class="ez-diff__head">
		<code>{file.filename}</code>
		<span class="ez-diff__add">+{file.additions}</span>
		<span class="ez-diff__del">-{file.deletions}</span>
		<span class="ez-muted">{file.status}</span>
	</summary>
	{#if skipped}
		<p class="ez-muted ez-diff__skipped">
			{skipped[0]!.toUpperCase() + skipped.slice(1)}.
			<a href="{url}/files" target="_blank" rel="noopener noreferrer">View on GitHub</a>
		</p>
	{:else}
		<table class="ez-diff__table" aria-label="Changes in {file.filename}">
			<tbody>
				{#each lines as l, i (i)}
					<tr class="ez-diff__line ez-diff__line--{l.kind}">
						<td class="ez-diff__no" aria-hidden="true">{l.oldNo ?? ''}</td>
						<td class="ez-diff__no" aria-hidden="true">{l.newNo ?? ''}</td>
						<td class="ez-diff__text"
							><span class="ez-diff__sign"
								>{l.kind === 'add' ? '+' : l.kind === 'del' ? '-' : ' '}</span
							>{l.text}</td
						>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</details>
