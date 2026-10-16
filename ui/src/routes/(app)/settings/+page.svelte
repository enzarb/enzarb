<script lang="ts">
	import ClaudeSetup from '#lib/components/ClaudeSetup.svelte';
	import GitHubInstallations from '#lib/components/GitHubInstallations.svelte';
	import NotificationSettings from '#lib/components/NotificationSettings.svelte';
	import KeyValueList from '#lib/components/ui/KeyValueList.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import { invalidateAll } from '$app/navigation';
	import { refreshRepositoryChoices } from '#lib/projects.remote.ts';
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
	refresh={async () => {
		await refreshRepositoryChoices();
		await invalidateAll();
	}}
	installations={data.installations}
	fetchedAt={data.fetchedAt}
/>

<div id="notifications">
	<Section
		title="Notifications"
		description="Get a push notification on this device when something needs you, so you do not have to watch the page."
	>
		<NotificationSettings />
	</Section>
</div>

<div id="claude">
	<Section
		title="Claude"
		description="One Claude token for all your Projects, used by chats and workflow runs."
	>
		<ClaudeSetup sandboxes={data.sandboxes} />
	</Section>
</div>
