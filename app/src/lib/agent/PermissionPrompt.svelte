<script lang="ts">
	import type { PermissionOptionPayload } from './types';
	import Markdown from './Markdown.svelte';

	let {
		title,
		options,
		plan = null,
		onRespond
	}: {
		title: string;
		options: PermissionOptionPayload[];
		plan?: string | null;
		onRespond: (optionId: string) => void;
	} = $props();

	function kindClass(kind: string) {
		return kind.startsWith('allow') ? 'allow' : 'reject';
	}
</script>

<div class="perm-prompt">
	<div class="perm-title">Permission requested: <strong>{title}</strong></div>
	{#if plan}
		<div class="perm-plan"><Markdown text={plan} /></div>
	{/if}
	<div class="perm-actions">
		{#each options as opt (opt.option_id)}
			<button class="perm-btn {kindClass(opt.kind)}" onclick={() => onRespond(opt.option_id)}>
				{opt.label}
			</button>
		{/each}
	</div>
</div>

<style>
	.perm-prompt { border: 1px solid #f5a623; border-radius: 6px; padding: 0.6rem 0.8rem; background: color-mix(in srgb, #f5a623 10%, transparent); font-size: 12px; }
	.perm-title { margin-bottom: 0.5rem; }
	.perm-plan { margin-bottom: 0.6rem; padding: 0.5rem 0.6rem; background: var(--color-surface); border: 1px solid var(--color-border); border-radius: 4px; font-size: 12px; line-height: 1.5; overflow-y: auto; max-height: 480px; }
	.perm-actions { display: flex; gap: 0.4rem; flex-wrap: wrap; }
	.perm-btn { font-size: 12px; padding: 0.3rem 0.7rem; border-radius: 4px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text); cursor: pointer; }
	.perm-btn.allow { border-color: #3fb950; color: #3fb950; }
	.perm-btn.allow:hover { background: color-mix(in srgb, #3fb950 15%, transparent); }
	.perm-btn.reject { border-color: var(--color-danger); color: var(--color-danger); }
	.perm-btn.reject:hover { background: color-mix(in srgb, var(--color-danger) 15%, transparent); }
</style>
