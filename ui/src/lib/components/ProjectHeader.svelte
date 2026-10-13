<script lang="ts">
	import ProjectNav from '#lib/components/ProjectNav.svelte';
	import Tooltip from '#lib/components/ui/Tooltip.svelte';

	const NAME_HINT =
		"The Project's Kubernetes name, used in URLs and keys. It can't be changed, unlike the display name.";
	const SANDBOX_HINT = "The sandbox pod that runs this Project's agents, terminal and chat.";

	/** One-row project header: title and subtext on the left, the tabs on the right. */
	let {
		displayName,
		name,
		sandboxName,
		pathname
	}: { displayName: string; name: string; sandboxName?: string | null; pathname: string } =
		$props();
</script>

<header class="ez-project-header">
	<div class="ez-project-header__text">
		<h1>{displayName}</h1>
		<div class="ez-cluster ez-project-header__meta">
			<Tooltip text={NAME_HINT} side="bottom">
				{#snippet trigger(props)}
					<!-- svelte-ignore a11y_no_noninteractive_tabindex (tooltips need a focusable trigger) -->
					<code {...props} tabindex="0">{name}</code>
				{/snippet}
			</Tooltip>
			{#if sandboxName}
				<span class="ez-muted">
					<Tooltip text={SANDBOX_HINT} side="bottom">
						{#snippet trigger(props)}
							<!-- svelte-ignore a11y_no_noninteractive_tabindex (tooltips need a focusable trigger) -->
							<span {...props} tabindex="0">sandbox</span>
						{/snippet}
					</Tooltip>
					<Tooltip text={SANDBOX_HINT} side="bottom">
						{#snippet trigger(props)}
							<!-- svelte-ignore a11y_no_noninteractive_tabindex (tooltips need a focusable trigger) -->
							<code {...props} tabindex="0">{sandboxName}</code>
						{/snippet}
					</Tooltip>
				</span>
			{/if}
		</div>
	</div>
	<ProjectNav {name} {pathname} />
</header>
