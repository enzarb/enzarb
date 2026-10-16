<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import TextField from '#lib/components/form/TextField.svelte';
	import { SECRET_NAME, type SecretName } from '#lib/tasks.ts';

	/**
	 * Named secrets of one scope (user or project). Only names are ever read back; a value is sent
	 * once on save and never shown again.
	 */
	let {
		secrets,
		onset,
		ondelete
	}: {
		secrets: SecretName[];
		onset: (name: string, value: string) => Promise<unknown>;
		ondelete: (name: string) => Promise<unknown>;
	} = $props();

	let name = $state('');
	let value = $state('');
	let pending = $state(false);
	let message = $state('');
	const valid = $derived(SECRET_NAME.test(name) && value.length > 0);

	async function save(e: SubmitEvent) {
		e.preventDefault();
		if (!valid) return;
		pending = true;
		message = '';
		try {
			await onset(name, value);
			message = `Saved ${name}.`;
			name = '';
			value = '';
		} catch {
			message = 'Could not save the secret.';
		} finally {
			pending = false;
		}
	}

	async function remove(n: string) {
		message = '';
		try {
			await ondelete(n);
			message = `Deleted ${n}.`;
		} catch {
			message = `Could not delete ${n}.`;
		}
	}
</script>

<div class="ez-stack">
	{#if secrets.length}
		<ul class="ez-secrets" aria-label="Secrets">
			{#each secrets as s (s.name)}
				<li>
					<code>{s.name}</code>
					<span class="ez-muted">updated {new Date(s.updatedAt).toLocaleDateString()}</span>
					<Button
						size="sm"
						variant="ghost"
						aria-label="Delete {s.name}"
						onclick={() => remove(s.name)}>Delete</Button
					>
				</li>
			{/each}
		</ul>
	{:else}
		<p class="ez-muted">No secrets yet.</p>
	{/if}
	<form class="ez-stack" onsubmit={save}>
		<TextField
			label="Name"
			help="Upper case, digits and _ (e.g. NPM_TOKEN). Saving an existing name replaces it."
			bind:value={name}
			autocomplete="off"
		/>
		<TextField label="Value" type="password" bind:value autocomplete="new-password" />
		<div class="ez-cluster">
			<Button type="submit" variant="primary" loading={pending} disabled={!valid}
				>Save secret</Button
			>
			{#if message}<span class="ez-muted" role="status">{message}</span>{/if}
		</div>
	</form>
</div>
