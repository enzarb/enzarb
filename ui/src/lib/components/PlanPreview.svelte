<script lang="ts">
	/** Collapsible plan; `load` runs on first open and returns sanitized HTML (or null for none). */
	let { load }: { load: () => Promise<string | null> } = $props();

	let html = $state<string | null>(null);
	let status = $state<'idle' | 'loading' | 'done' | 'error'>('idle');

	async function toggle(e: Event) {
		if (!(e.currentTarget as HTMLDetailsElement).open || status !== 'idle') return;
		status = 'loading';
		try {
			html = await load();
			status = 'done';
		} catch {
			status = 'error';
		}
	}
</script>

<details class="ez-plan" ontoggle={toggle}>
	<summary>Plan</summary>
	{#if status === 'loading'}
		<p class="ez-muted">Loading plan…</p>
	{:else if status === 'error'}
		<p class="ez-muted">Could not load the plan.</p>
	{:else if status === 'done'}
		{#if html}
			<!-- Sanitized on the server (sanitize-html allow-list) before it is sent. -->
			<!-- eslint-disable-next-line svelte/no-at-html-tags -->
			<div class="ez-markdown">{@html html}</div>
		{:else}
			<p class="ez-muted">No plan yet.</p>
		{/if}
	{/if}
</details>
