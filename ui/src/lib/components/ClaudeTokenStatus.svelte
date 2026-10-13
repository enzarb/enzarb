<script lang="ts">
	import Badge from './ui/Badge.svelte';

	/** Presence of the user's Claude token, from `app_api.claude_credential_v`. */
	let { configured, updatedAt = null }: { configured: boolean; updatedAt?: string | null } =
		$props();
</script>

<p class="ez-cluster">
	Claude token:
	{#if configured}
		<Badge tone="success">Configured</Badge>
		{#if updatedAt}<span class="ez-muted">since {new Date(updatedAt).toLocaleString()}</span>{/if}
	{:else}
		<Badge tone="neutral" hollow>Not configured</Badge>
	{/if}
</p>
