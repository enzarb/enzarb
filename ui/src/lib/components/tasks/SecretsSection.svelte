<script lang="ts">
	import Notice from '#lib/components/ui/Notice.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import { deleteSecret, listSecrets, setSecret } from '#lib/tasks.remote.ts';
	import SecretsForm from './SecretsForm.svelte';

	/** Secrets of the user (no `project`) or of one project; steps request them by name. */
	let { project }: { project?: string } = $props();

	const secrets = $derived(listSecrets(project));
</script>

<Section
	title="Secrets"
	description={project
		? 'Available to steps of this project that request them by name. Values are encrypted and never shown again.'
		: 'Yours, available to steps of your projects that request them by name. Values are encrypted and never shown again.'}
>
	{#if secrets.error && !secrets.current}
		<Notice tone="error">Could not load secrets.</Notice>
	{:else}
		<SecretsForm
			secrets={secrets.current ?? []}
			onset={(name, value) => setSecret({ project, name, value })}
			ondelete={(name) => deleteSecret({ project, name })}
		/>
	{/if}
</Section>
