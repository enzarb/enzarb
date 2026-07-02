<script lang="ts">
	import type { PermissionOptionPayload } from './types';

	let {
		title,
		options,
		onRespond
	}: {
		title: string;
		options: PermissionOptionPayload[];
		onRespond: (optionId: string) => void;
	} = $props();

	function kindClass(kind: string) {
		return kind.startsWith('allow') ? 'allow' : 'reject';
	}
</script>

<div class="perm-prompt">
	<div class="perm-title">Permission requested: <strong>{title}</strong></div>
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
	.perm-actions { display: flex; gap: 0.4rem; flex-wrap: wrap; }
	.perm-btn { font-size: 12px; padding: 0.3rem 0.7rem; border-radius: 4px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text); cursor: pointer; }
	.perm-btn.allow { border-color: #3fb950; color: #3fb950; }
	.perm-btn.allow:hover { background: color-mix(in srgb, #3fb950 15%, transparent); }
	.perm-btn.reject { border-color: var(--color-danger); color: var(--color-danger); }
	.perm-btn.reject:hover { background: color-mix(in srgb, var(--color-danger) 15%, transparent); }
</style>
