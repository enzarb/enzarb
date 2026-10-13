<script lang="ts">
	import { toasts, type ToastItem } from '#lib/toast.svelte.ts';
	import Button from '../form/Button.svelte';
	import Icon, { type IconName } from './Icon.svelte';

	/**
	 * Renders the toast queue. Mount once (root layout). The polite region (role=status) exists from the
	 * first render, because screen readers only announce changes to a region already in the page;
	 * info, success and warning go there. Errors get a role=alert block while one is showing.
	 */
	const icons: Record<ToastItem['tone'], IconName> = {
		info: 'info',
		success: 'check',
		warning: 'alert',
		error: 'alert-circle'
	};
	const polite = $derived(toasts.items.filter((t) => t.tone !== 'error'));
	const assertive = $derived(toasts.items.filter((t) => t.tone === 'error'));
</script>

{#snippet toast(t: ToastItem)}
	<div class="ez-toast ez-toast--{t.tone}">
		<span class="ez-toast__icon"><Icon name={icons[t.tone]} /></span>
		<div>
			<div class="ez-toast__title">{t.title}</div>
			{#if t.description}<div class="ez-toast__description">{t.description}</div>{/if}
		</div>
		<Button
			variant="ghost"
			size="sm"
			icon
			aria-label="Dismiss notification"
			onclick={() => toasts.dismiss(t.id)}><Icon name="x" /></Button
		>
	</div>
{/snippet}

<div class="ez-toasts">
	<div
		role="status"
		aria-live="polite"
		aria-atomic="false"
		class="ez-stack"
		style="--ez-gap: var(--ez-space-2)"
	>
		{#each polite as t (t.id)}{@render toast(t)}{/each}
	</div>
	<!-- An alert is announced when inserted, so this region exists only while an error does. -->
	{#if assertive.length}
		<div role="alert" class="ez-stack" style="--ez-gap: var(--ez-space-2)">
			{#each assertive as t (t.id)}{@render toast(t)}{/each}
		</div>
	{/if}
</div>
