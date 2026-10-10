<script lang="ts">
	import PageHeading from '#lib/components/PageHeading.svelte';
	import Button from '#lib/components/form/Button.svelte';
	import Checkbox from '#lib/components/form/Checkbox.svelte';
	import TextField from '#lib/components/form/TextField.svelte';
	import RepoListInput from '#lib/components/RepoListInput.svelte';
	import { createProject } from '#lib/projects.remote.ts';

	const fields = createProject.fields;
</script>

<svelte:head><title>New project · enzarb</title></svelte:head>

<PageHeading title="New project" />

<form {...createProject} class="stack card">
	<TextField
		label="Name"
		placeholder="my-project"
		{...fields.name.as('text')}
		issues={fields.name.issues()}
	/>
	<TextField label="Display name" {...fields.displayName.as('text')} />

	<RepoListInput
		field={(i, f) => fields.repositories[i][f].as(f === 'url' ? 'url' : 'text')}
		issues={(i, f) => (fields.repositories[i]?.[f]?.issues() ?? []).map((x) => x.message)}
	/>
	{#each fields.repositories.issues() ?? [] as issue (issue.message)}
		<p class="field-error">{issue.message}</p>
	{/each}

	<fieldset class="checks">
		<legend>Triggers</legend>
		<Checkbox label="Issues" name={fields.issues.as('checkbox').name} checked />
		<Checkbox label="Change requests" name={fields.changeRequests.as('checkbox').name} checked />
		<Checkbox label="Mentions" name={fields.mentions.as('checkbox').name} checked />
	</fieldset>

	<Button variant="primary" type="submit" disabled={createProject.pending > 0}
		>Create project</Button
	>
</form>
