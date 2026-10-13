<script lang="ts">
	import type { Snippet } from 'svelte';
	import Icon, { type IconName } from './Icon.svelte';

	type NoticeTone = 'info' | 'warning' | 'error' | 'success';
	const icons: Record<NoticeTone, IconName> = {
		info: 'info',
		warning: 'alert',
		error: 'alert-circle',
		success: 'check'
	};

	/**
	 * Inline banner. info/success/warning are announced politely (role=status), error assertively
	 * (role=alert). `title` is optional bold lead-in, `actions` holds buttons or links.
	 */
	let {
		tone = 'info',
		title,
		actions,
		children
	}: { tone?: NoticeTone; title?: string; actions?: Snippet; children: Snippet } = $props();
</script>

<div
	class="ez-notice"
	class:ez-notice--warning={tone === 'warning'}
	class:ez-notice--error={tone === 'error'}
	class:ez-notice--success={tone === 'success'}
	role={tone === 'error' ? 'alert' : 'status'}
>
	<span class="ez-notice__icon"><Icon name={icons[tone]} /></span>
	<div class="ez-notice__body">
		{#if title}<strong class="ez-notice__title">{title}</strong>{/if}
		<div>{@render children()}</div>
		{#if actions}<div class="ez-notice__actions ez-cluster">{@render actions()}</div>{/if}
	</div>
</div>
