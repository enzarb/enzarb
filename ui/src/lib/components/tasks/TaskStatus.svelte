<script lang="ts">
	import Badge from '#lib/components/ui/Badge.svelte';
	import type { Tone } from '#lib/components/ui/status.ts';

	/** The engine phase (`running`, `waiting`, `paused`, `ended`) and the YAML end status. */
	let { status, endStatus = null }: { status: string; endStatus?: string | null } = $props();

	const TONES: Record<string, Tone> = {
		running: 'info',
		waiting: 'warning',
		paused: 'danger',
		ended: 'neutral'
	};
	const tone = $derived(TONES[status] ?? 'neutral');
</script>

<Badge {tone} hollow={status === 'waiting'}
	>{status === 'ended' && endStatus ? `ended: ${endStatus}` : status}</Badge
>
