<script lang="ts">
	/** Round user image, or the first letter of the login when there is none (or it fails to load). */
	let { login, url }: { login: string; url: string | null } = $props();
	let failed = $state(false);
</script>

<span class="ez-avatar">
	{#if url && !failed}
		<img src={url} alt="" width="28" height="28" onerror={() => (failed = true)} />
	{:else}
		<span aria-hidden="true">{login.slice(0, 1)}</span>
	{/if}
</span>
