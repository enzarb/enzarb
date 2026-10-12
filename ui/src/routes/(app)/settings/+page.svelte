<script lang="ts">
	import GitHubInstallations from '#lib/components/GitHubInstallations.svelte';
	import KeyValueList from '#lib/components/ui/KeyValueList.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<PageHeader
	title="Settings"
	description="Your GitHub account and the repositories enzarb can reach."
/>

<KeyValueList
	items={[
		{ term: 'GitHub account', value: data.account.login },
		...(data.account.name ? [{ term: 'Name', value: data.account.name }] : [])
	]}
/>

<GitHubInstallations
	installUrl={data.installUrl}
	refreshHref="/login/github?refresh=1&return=%2Fsettings"
	installations={data.installations}
	fetchedAt={data.fetchedAt}
/>
