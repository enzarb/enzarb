<script lang="ts">
	import { onMount } from 'svelte';
	import { theme } from '#lib/theme.svelte.ts';
	import { isThemeChoice } from '#lib/theme.ts';
	import Avatar from './ui/Avatar.svelte';
	import DropdownMenu from './ui/DropdownMenu.svelte';
	import Icon from './ui/Icon.svelte';

	let { login, name, avatarUrl }: { login: string; name: string | null; avatarUrl: string | null } =
		$props();

	let logout = $state<HTMLFormElement>();
	onMount(() => theme.sync());
</script>

<DropdownMenu
	label="Account menu for {login}"
	entries={[
		{
			type: 'radio',
			label: 'Theme',
			value: theme.choice,
			onchange: (v) => isThemeChoice(v) && theme.set(v),
			options: [
				{ value: 'system', label: 'System', icon: 'monitor' },
				{ value: 'light', label: 'Light', icon: 'sun' },
				{ value: 'dark', label: 'Dark', icon: 'moon' }
			]
		},
		{ type: 'separator' },
		{ type: 'item', label: 'Settings', icon: 'settings', href: '/settings' },
		{ type: 'separator' },
		{ type: 'item', label: 'Sign out', icon: 'logout', onselect: () => logout?.requestSubmit() }
	]}
>
	{#snippet trigger()}
		<Avatar {login} url={avatarUrl} />
		<span>{login}</span>
		<Icon name="chevron-down" />
	{/snippet}
	{#snippet header()}
		{#if name}<strong>{name}</strong>{/if}
		<span class="ez-muted">@{login}</span>
	{/snippet}
</DropdownMenu>

<form bind:this={logout} method="POST" action="/logout" hidden></form>
<!-- Without JavaScript the menu cannot open, so offer a plain sign-out form. -->
<noscript>
	<a class="ez-btn ez-btn--secondary ez-btn--sm" href="/settings">Settings</a>
	<form method="POST" action="/logout">
		<button class="ez-btn ez-btn--secondary ez-btn--sm" type="submit">Sign out</button>
	</form>
</noscript>
