<script lang="ts">
	import { currentUser } from '#lib/user.remote.ts';
	import Logo from './Logo.svelte';
	import UserMenu from './UserMenu.svelte';
</script>

<header class="ez-header">
	<Logo />
	<svelte:boundary>
		<UserMenu {...await currentUser()} />
	</svelte:boundary>
</header>
