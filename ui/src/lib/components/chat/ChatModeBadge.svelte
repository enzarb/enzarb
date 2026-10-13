<script lang="ts">
	import Badge from '#lib/components/ui/Badge.svelte';
	import type { ModeId } from '#lib/acp/types.ts';

	/** The permission mode as text; the bypass tone only adds to the words. */
	let { modeId }: { modeId: ModeId } = $props();

	const LABELS: Record<ModeId, string> = {
		bypassPermissions: 'Bypass – no permission prompts',
		default: 'Default',
		plan: 'Plan'
	};
</script>

<span class="ez-chat-mode" class:ez-chat-mode--bypass={modeId === 'bypassPermissions'}>
	<Badge tone={modeId === 'bypassPermissions' ? 'warning' : 'neutral'}>{LABELS[modeId]}</Badge>
</span>
