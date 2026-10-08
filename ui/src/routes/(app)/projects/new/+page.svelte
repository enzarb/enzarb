<script lang="ts">
	import PageHeading from '#lib/components/PageHeading.svelte';
	import DockerOptions from '#lib/components/DockerOptions.svelte';
	import RepoListInput from '#lib/components/RepoListInput.svelte';
	import { createProject } from '#lib/projects.remote.ts';

	const fields = createProject.fields;
</script>

<svelte:head><title>New project · enzarb</title></svelte:head>

<PageHeading title="New project" />

<form {...createProject} class="stack card">
	<label>
		Name
		<input {...fields.name.as('text')} placeholder="my-project" />
		{#each fields.name.issues() ?? [] as issue (issue.message)}
			<p class="field-error">{issue.message}</p>
		{/each}
	</label>
	<label>
		Display name
		<input {...fields.displayName.as('text')} />
	</label>

	<RepoListInput
		issues={(i, f) => (fields.repositories[i]?.[f]?.issues() ?? []).map((x) => x.message)}
	/>
	{#each fields.repositories.issues() ?? [] as issue (issue.message)}
		<p class="field-error">{issue.message}</p>
	{/each}

	<fieldset class="checks">
		<legend>Triggers</legend>
		<label><input {...fields.issues.as('checkbox')} checked /> Issues</label>
		<label><input {...fields.changeRequests.as('checkbox')} checked /> Change requests</label>
		<label><input {...fields.mentions.as('checkbox')} checked /> Mentions</label>
	</fieldset>

	<DockerOptions {fields} />

	<button class="primary" type="submit" disabled={createProject.pending > 0}>Create project</button>
</form>
