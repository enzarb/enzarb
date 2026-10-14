<script lang="ts">
	import type { ChartRollout } from '#lib/about-types.ts';
	import { displayVersion, normalizeTag } from '#lib/versions.ts';
	import Notice from './ui/Notice.svelte';

	let { rollout }: { rollout: ChartRollout | null | undefined } = $props();

	const behind = $derived(
		!!rollout?.installed &&
			!!rollout.latest &&
			normalizeTag(rollout.installed) !== normalizeTag(rollout.latest)
	);
</script>

{#if rollout}
	{#if rollout.stuck}
		<Notice tone="warning" title="Chart upgrade failing">
			The helm install job cannot reach <code>{displayVersion(rollout.desired)}</code>; oldie still
			runs <code>{displayVersion(rollout.installed)}</code>. Check the <code>helm-install</code> job
			logs in <code>kube-system</code>.
		</Notice>
	{/if}
	<p>
		Chart installed <code>{displayVersion(rollout.installed)}</code> / latest
		<code>{displayVersion(rollout.latest)}</code>
		{#if behind && !rollout.stuck}<span class="ez-muted">(upgrade pending)</span>{/if}
	</p>
{/if}
