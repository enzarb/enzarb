<script lang="ts">
	let { login, name, avatarUrl }: { login: string; name: string | null; avatarUrl: string | null } =
		$props();
</script>

<details class="user-menu">
	<summary>
		{#if avatarUrl}<img class="avatar" src={avatarUrl} alt="" width="28" height="28" />{/if}
		<span>{login}</span>
	</summary>
	<div class="menu card stack">
		<div>
			{#if name}<strong>{name}</strong><br />{/if}
			<span class="muted">@{login}</span>
		</div>
		<form method="POST" action="/logout">
			<button type="submit">Sign out</button>
		</form>
	</div>
</details>
