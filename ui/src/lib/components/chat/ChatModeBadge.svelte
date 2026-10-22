<script lang="ts">
	import Badge from '#lib/components/ui/Badge.svelte';
	import { BYPASS_MODE, CLAUDE_MODES, type ModeId } from '#lib/acp/types.ts';

	/** The active mode as text; the bypass tone only adds to the words. */
	let { modeId, name }: { modeId: ModeId; name?: string } = $props();

	const label = $derived(
		modeId === BYPASS_MODE
			? 'Bypass – no permission prompts'
			: (name ?? CLAUDE_MODES.find((m) => m.id === modeId)?.name ?? modeId)
	);
</script>

<span class="ez-chat-mode" class:ez-chat-mode--bypass={modeId === BYPASS_MODE}>
	<Badge tone={modeId === BYPASS_MODE ? 'warning' : 'neutral'}>{label}</Badge>
</span>
