<script lang="ts">
	import { untrack } from 'svelte';
	import {
		browserTimers,
		browserVisibility,
		createChatSession
	} from '#lib/acp/chat-session.svelte.ts';
	import type { ConnectionDeps } from '#lib/acp/connection.ts';
	import { sandboxConnector, type MuxSession } from '#lib/acp/sandbox-connect.ts';
	import Notice from '#lib/components/ui/Notice.svelte';
	import { terminalConnection } from '#lib/terminal.remote.ts';
	import ChatStatus from './ChatStatus.svelte';
	import ChatTranscript from './ChatTranscript.svelte';
	import Composer from './Composer.svelte';

	let {
		project,
		connect
	}: {
		project: string;
		/** Override the sandbox connection (tests); defaults to the terminal's token + websocket path. */
		connect?: ConnectionDeps['connect'];
	} = $props();

	// One connection per mount: the page keys this component by project name.
	const connector = sandboxConnector(
		untrack(() => project),
		{
			connection: (name) => terminalConnection(name),
			openSocket: (url) => new WebSocket(url),
			handshake: async (ws) => {
				const proto = await import('@enzarb/proto');
				return (await proto.Session.connect(ws, { client: 'ui' })) as unknown as MuxSession;
			}
		}
	);

	const chat = createChatSession({
		connect: untrack(() => connect) ?? connector.connect,
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
