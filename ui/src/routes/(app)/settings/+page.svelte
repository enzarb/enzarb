<script lang="ts">
	import ClaudeSetup from '#lib/components/ClaudeSetup.svelte';
	import GitHubInstallations from '#lib/components/GitHubInstallations.svelte';
	import KeyValueList from '#lib/components/ui/KeyValueList.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<PageHeader
	title="Settings"
	description="Your GitHub account, the repositories enzarb can reach and your Claude token."
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

<div id="claude">
	<Section
		title="Claude"
		description="One Claude token for all your Projects, used by chats and workflow runs."
	>
		<ClaudeSetup sandboxes={data.sandboxes} />
	</Section>
</div>
