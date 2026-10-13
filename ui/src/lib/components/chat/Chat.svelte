<script lang="ts">
	import { untrack } from 'svelte';
	import {
		browserTimers,
		browserVisibility,
		createChatSession
	} from '#lib/acp/chat-session.svelte.ts';
	import type { ConnectionDeps } from '#lib/acp/connection.ts';
	import { sandboxConnector, type MuxSession } from '#lib/acp/sandbox-connect.ts';
	import * as sessionStore from '#lib/acp/session-store.ts';
	import Notice from '#lib/components/ui/Notice.svelte';
	import { chatConnection } from '#lib/chat.remote.ts';
	import ChatStatus from './ChatStatus.svelte';
	import ChatTranscript from './ChatTranscript.svelte';
	import Composer from './Composer.svelte';

	let {
		project,
		connect
	}: {
		project: string;
		/** Override the sandbox connection (tests); defaults to the chat's token + websocket path. */
		connect?: ConnectionDeps['connect'];
	} = $props();

	// One connection per mount: the page keys this component by project name.
	const connector = sandboxConnector(
		untrack(() => project),
		{
			// untracked: a client `command()` bumps a $state synchronously, which would re-run the start
			// effect below; its cleanup disposes the connection and the chat stalls at "Session token"
			connection: (name) => untrack(() => chatConnection(name)),
			openSocket: (url) => new WebSocket(url),
			handshake: async (ws) => {
				const proto = await import('@enzarb/proto');
				return (await proto.Session.connect(ws, { client: 'ui' })) as unknown as MuxSession;
			}
		}
	);

	const chat = createChatSession({
		connect: untrack(() => connect) ?? connector.connect,
		initialSessionId: sessionStore.load(untrack(() => project)),
		onSessionId: (id) => {
			const name = untrack(() => project);
			if (id) sessionStore.save(name, id);
			else sessionStore.clear(name);
		},
		...browserTimers,
		visibility: browserVisibility
	});

	$effect(() => {
		chat.start();
		return () => {
			chat.dispose();
			connector.dispose();
		};
	});

	const st = $derived(chat.state);
	const onAction = (a: 'retry' | 'reconnect' | 'take_over') => {
		void a;
		chat.reconnect();
	};
</script>

<div class="ez-chat" data-phase={st.phase}>
	{#if st.phase === 'unavailable'}
		<Notice tone="info" title="Chat not available">
			This sandbox or its proxy is too old for chat. It becomes available once both are updated and
			the sandbox has restarted.
		</Notice>
	{:else}
		<div class="ez-chat-header">
			<button
				type="button"
				class="ez-btn ez-btn--secondary ez-btn--sm"
				onclick={() => chat.newChat()}>New chat</button
			>
		</div>
		{#if st.phase !== 'ready'}
			<ChatStatus progress={st.progress} message={st.message} action={st.action} {onAction} />
		{/if}
		<ChatTranscript items={st.chat.items} epoch={st.epoch} onAnswer={chat.answer} />
		{#if st.error}<p class="ez-muted" role="alert">{st.error}</p>{/if}
		<Composer
			busy={st.busy}
			disabled={st.phase !== 'ready' && st.phase !== 'disconnected'}
			onSend={chat.send}
			onStop={chat.stop}
		/>
	{/if}
</div>
