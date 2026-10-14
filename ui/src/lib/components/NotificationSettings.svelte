<script lang="ts">
	import { onMount } from 'svelte';
	import {
		getPushState,
		sendTest,
		subscribe,
		unsubscribe,
		updateCategories
	} from '#lib/push.remote.ts';
	import { InstallPrompt } from '#lib/install-prompt.svelte.ts';
	import { CATEGORY_LABELS } from '#lib/push.ts';
	import {
		browserEnv,
		currentSubscription,
		detectSupport,
		subscribeThisDevice,
		unsubscribeThisDevice,
		type PushSupport
	} from '#lib/push-client.ts';
	import { NEEDS_YOU_CATEGORIES, type NeedsYouCategory } from '#lib/work-items.ts';
	import Button from './form/Button.svelte';
	import Notice from './ui/Notice.svelte';

	/**
	 * Per-device notifications. A device is the browser this page runs in: enabling subscribes it
	 * and registers it for the signed-in user; the categories decide which "needs you" events
	 * reach it. The server only sends for Projects the user owns.
	 */
	const push = $derived(getPushState());
	const install = new InstallPrompt();

	let support = $state<PushSupport | 'checking'>('checking');
	let endpoint = $state<string | null>(null);
	let busy = $state(false);
	let message = $state<{ tone: 'error' | 'success'; text: string } | null>(null);

	onMount(() => {
		const env = browserEnv();
		support = detectSupport(env);
		if (support === 'ok') {
			currentSubscription()
				.then((s) => (endpoint = s?.endpoint ?? null))
				.catch(() => {});
		}
		return install.start();
	});

	const device = $derived(push.current?.devices.find((d) => d.endpoint === endpoint));
	const others = $derived(push.current?.devices.filter((d) => d.endpoint !== endpoint) ?? []);

	async function run(action: () => Promise<void>, ok?: string) {
		busy = true;
		message = null;
		try {
			await action();
			await push.refresh();
			if (ok) message = { tone: 'success', text: ok };
		} catch (e) {
			message = {
				tone: 'error',
				text: e instanceof Error ? e.message : 'Something went wrong. Try again.'
			};
		} finally {
			busy = false;
		}
	}

	const enable = () =>
		run(async () => {
			const key = push.current?.publicKey;
			if (!key) throw new Error('Notifications are not configured on this server.');
			const sub = await subscribeThisDevice(key);
			endpoint = sub.endpoint;
			await subscribe({ ...sub, categories: [...NEEDS_YOU_CATEGORIES] });
		});

	const disable = () =>
		run(async () => {
			const ep = endpoint;
			await unsubscribeThisDevice();
			if (ep) await unsubscribe(ep);
			endpoint = null;
		});

	function toggle(category: NeedsYouCategory, on: boolean) {
		if (!device || !endpoint) return;
		const next = NEEDS_YOU_CATEGORIES.filter((c) =>
			c === category ? on : device.categories.includes(c)
		);
		return run(() => updateCategories({ endpoint: endpoint!, categories: next }));
	}

	const test = () =>
		run(async () => {
			await sendTest(endpoint!);
		}, 'Test notification sent.');

	const removeOther = (ep: string) => run(() => unsubscribe(ep));
</script>

<div class="ez-stack">
	{#if push.error && !push.current}
		<Notice tone="error">Could not read your notification settings.</Notice>
	{:else if push.current && !push.current.publicKey}
		<Notice tone="info">Push notifications are not configured on this server.</Notice>
	{:else if support === 'needs-install'}
		<Notice tone="info" title="Add enzarb to your home screen first">
			On iPhone and iPad (iOS/iPadOS 16.4 or later) notifications only work for an app added to the
			home screen. In Safari choose Share, then Add to Home Screen, open enzarb from there and come
			back to this page.
		</Notice>
	{:else if support === 'unsupported'}
		<Notice tone="info">This browser cannot receive push notifications.</Notice>
	{:else if support === 'denied'}
		<Notice tone="warning">
			Notifications are blocked for this site. Allow them in your browser's site settings, then
			reload.
		</Notice>
	{:else if support === 'ok'}
		{#if device}
			<p class="ez-muted">Notifications are on for this device. Tell me when:</p>
			<fieldset class="ez-stack" style="--ez-gap: var(--ez-space-2)" disabled={busy}>
				<legend class="ez-sr-only">Events</legend>
				{#each NEEDS_YOU_CATEGORIES as category (category)}
					<label class="ez-cluster">
						<input
							type="checkbox"
							checked={device.categories.includes(category)}
							onchange={(e) => toggle(category, e.currentTarget.checked)}
						/>
						{CATEGORY_LABELS[category]}
					</label>
				{/each}
			</fieldset>
			<div class="ez-cluster">
				<Button loading={busy} onclick={test}>Send a test</Button>
				<Button variant="danger" loading={busy} onclick={disable}>Turn off for this device</Button>
			</div>
		{:else}
			<p class="ez-muted">
				Get a notification on this device when a work item needs you: a plan to approve, a change
				request to merge, a blocked change request, or a failure.
			</p>
			<div class="ez-cluster">
				<Button variant="primary" loading={busy} onclick={enable}>Turn on for this device</Button>
			</div>
		{/if}
		{#if others.length > 0}
			<p class="ez-muted">Other devices</p>
			<ul class="ez-stack" style="--ez-gap: var(--ez-space-2)">
				{#each others as d (d.endpoint)}
					<li class="ez-cluster">
						<span>{d.userAgent ?? 'Unknown device'}</span>
						<Button
							size="sm"
							variant="ghost"
							disabled={busy}
							onclick={() => removeOther(d.endpoint)}
						>
							Remove
						</Button>
					</li>
				{/each}
			</ul>
		{/if}
		{#if install.available}
			<div class="ez-cluster">
				<Button onclick={() => install.install()}>Install the app</Button>
				<Button variant="ghost" onclick={() => install.dismiss()}>Not now</Button>
			</div>
		{/if}
	{/if}
	{#if message}<Notice tone={message.tone}>{message.text}</Notice>{/if}
	<p class="ez-muted">
		On iPhone and iPad, notifications need iOS/iPadOS 16.4 or later and enzarb added to the home
		screen.
	</p>
</div>
