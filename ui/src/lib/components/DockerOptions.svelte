<script lang="ts">
	import { runtimeClasses } from '#lib/projects.remote.ts';
	import type { createProject } from '#lib/projects.remote.ts';

	let { fields }: { fields: typeof createProject.fields } = $props();

	let enabled = $state(false);
	const query = runtimeClasses();
	const classes = $derived(query.current ?? []);
</script>

<fieldset class="docker-options">
	<legend>Sandbox</legend>
	<label>
		<input
			{...fields.docker.as('checkbox')}
			onchange={(e) => (enabled = e.currentTarget.checked)}
		/> Docker in sandbox
	</label>
	{#if enabled}
		<label>
			Runtime class
			<select {...fields.runtimeClassName.as('select')} required>
				<option value="">Select isolation…</option>
				{#each classes as rc (rc)}
					<option value={rc}>{rc}</option>
				{/each}
			</select>
		</label>
		<p class="muted">Docker runs only inside gVisor or Kata isolation.</p>
		{#each fields.runtimeClassName.issues() ?? [] as issue (issue.message)}
			<p class="field-error">{issue.message}</p>
		{/each}
	{/if}
</fieldset>
