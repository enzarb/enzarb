<script lang="ts">
	// Test-only: RepoListInput inside a plain form, with the caller's field attributes.
	import RepoListInput from '../../RepoListInput.svelte';

	let { field }: { field: (i: number, f: 'url' | 'defaultBranch') => { name: string } } = $props();
	let posted = $state('');
</script>

<form
	onsubmit={(e) => {
		e.preventDefault();
		posted = JSON.stringify([...new FormData(e.currentTarget as HTMLFormElement).entries()]);
	}}
>
	<RepoListInput initial={[{ url: 'https://github.com/a/b', defaultBranch: 'dev' }]} {field} />
	<button type="submit">Send</button>
	<button type="reset">Reset</button>
</form>
<output data-testid="posted">{posted}</output>
