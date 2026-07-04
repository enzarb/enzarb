<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { AgentSocket, type ConnState } from '$lib/agent/agentSocket';
	import type {
		AcpWsEvent,
		ConfigOptionInfo,
		DiffPayload,
		PermissionOptionPayload,
		PlanEntryPayload,
		SessionModeInfo
	} from '$lib/agent/types';
	import { getAgentAuthToken } from '$lib/agentToken';
	import { workspaceHealth } from '$lib/workspaceHealth.svelte';
	import Markdown from '$lib/agent/Markdown.svelte';
	import ToolCallCard from '$lib/agent/ToolCallCard.svelte';
	import PlanView from '$lib/agent/PlanView.svelte';
	import PermissionPrompt from '$lib/agent/PermissionPrompt.svelte';

	type TimelineItem =
		| { kind: 'message'; role: 'user' | 'assistant'; text: string }
		| { kind: 'tool_call'; id: string; toolKind: string; title: string; status: string; diff: DiffPayload | null; output: string | null }
		| { kind: 'plan'; entries: PlanEntryPayload[] };

	type PendingPermission = {
		requestId: string;
		toolCallId: string;
		title: string;
		options: PermissionOptionPayload[];
	};

	interface Props {
		agentBase: string;
		namespace: string;
		project: string;
		sessionId: string;
	}

	let { agentBase, namespace, project, sessionId }: Props = $props();

	let socket: AgentSocket | undefined;
	let timeline: TimelineItem[] = $state([]);
	let pendingPermissions: PendingPermission[] = $state([]);
	let connState: ConnState = $state('connecting');
	let connectError = $state('');
	let draft = $state('');
	let mounted = false;
	let scrollEl: HTMLDivElement | undefined = $state();
	let textareaEl: HTMLTextAreaElement | undefined = $state();
	let availableModes: SessionModeInfo[] = $state([]);
	let currentMode: string = $state('default');
	let configOptions: ConfigOptionInfo[] = $state([]);

	const modelOption = $derived(
		configOptions.find((o) => o.category === 'model' || o.id === 'model') ?? null
	);

	async function loadSessionMeta() {
		if (!agentBase) return;
		await workspaceHealth(agentBase).ensureHealthy();
		const token = await getAgentAuthToken(namespace, project);
		if (!token) return;
		try {
			const res = await fetch(`${agentBase}/agent/sessions/${sessionId}`, {
				headers: { Authorization: `Bearer ${token}` }
			});
			if (res.ok) {
				const meta = await res.json();
				availableModes = meta.available_modes ?? [];
				currentMode = meta.mode_id ?? 'default';
				configOptions = meta.config_options ?? [];
			}
		} catch {}
	}

	function changeMode(modeId: string) {
		if (modeId === currentMode) return;
		currentMode = modeId;
		socket?.send({ type: 'set_permission_mode', mode_id: modeId });
	}

	function changeModel(value: string) {
		if (!modelOption || value === modelOption.current_value) return;
		configOptions = configOptions.map((o) =>
			o.id === modelOption.id ? { ...o, current_value: value } : o
		);
		socket?.send({ type: 'set_config_option', config_id: modelOption.id, value });
	}

	function handleEvent(event: AcpWsEvent) {
		switch (event.type) {
			case 'message_chunk': {
				const last = timeline[timeline.length - 1];
				if (last?.kind === 'message' && last.role === event.role) {
					last.text += event.text;
				} else {
					timeline.push({ kind: 'message', role: event.role, text: event.text });
				}
				break;
			}
			case 'tool_call_created':
				timeline.push({
					kind: 'tool_call',
					id: event.tool_call_id,
					toolKind: event.kind,
					title: event.title,
					status: event.status,
					diff: null,
					output: null
				});
				break;
			case 'tool_call_updated': {
				const item = [...timeline].reverse().find((t) => t.kind === 'tool_call' && t.id === event.tool_call_id);
				if (item && item.kind === 'tool_call') {
					if (event.status) item.status = event.status;
					if (event.diff) item.diff = event.diff;
					if (event.output) item.output = event.output;
				}
				break;
			}
			case 'plan_update': {
				const last = timeline[timeline.length - 1];
				if (last?.kind === 'plan') last.entries = event.entries;
				else timeline.push({ kind: 'plan', entries: event.entries });
				break;
			}
			case 'permission_request':
				pendingPermissions.push({
					requestId: event.request_id,
					toolCallId: event.tool_call_id,
					title: event.title,
					options: event.options
				});
				break;
			case 'permission_resolved':
				pendingPermissions = pendingPermissions.filter((p) => p.requestId !== event.request_id);
				break;
			case 'mode_changed':
				currentMode = event.mode_id;
				break;
			case 'config_options_changed':
				configOptions = event.config_options;
				break;
			case 'session_state':
				currentMode = event.mode_id ?? 'default';
				availableModes = event.available_modes;
				configOptions = event.config_options;
				break;
			case 'error':
				timeline.push({ kind: 'message', role: 'assistant', text: `⚠️ ${event.message}` });
				break;
		}
		scrollToBottom();
	}

	function scrollToBottom() {
		queueMicrotask(() => scrollEl?.scrollTo({ top: scrollEl.scrollHeight }));
	}

	function respondPermission(requestId: string, optionId: string) {
		socket?.send({ type: 'permission_response', request_id: requestId, option_id: optionId });
	}

	function sendMessage() {
		const text = draft.trim();
		if (!text || !socket) return;
		timeline.push({ kind: 'message', role: 'user', text });
		socket.send({ type: 'send_message', text });
		draft = '';
		if (textareaEl) { textareaEl.style.height = 'auto'; }
		scrollToBottom();
	}

	function growTextarea(el: HTMLTextAreaElement) {
		el.style.height = 'auto';
		el.style.height = el.scrollHeight + 'px';
	}

	onMount(async () => {
		mounted = true;
		if (!agentBase) return;
		await loadSessionMeta();
		if (!mounted) return;
		socket = new AgentSocket(
			agentBase,
			namespace,
			project,
			sessionId,
			handleEvent,
			(state, error) => {
				const reconnected = state === 'connected' && connState !== 'connected';
				connState = state;
				connectError = error;
				if (reconnected) void loadSessionMeta();
			},
			() => {
				// The server resends the full event history on every attach;
				// drop what we built up locally or it'll be duplicated.
				timeline = [];
				pendingPermissions = [];
			}
		);
		await socket.connect();
	});

	onDestroy(() => {
		mounted = false;
		socket?.close();
	});
</script>

<div class="agent-pane">
	<div class="pane-header">
		{#if connState !== 'connected'}
			<span class="conn-status {connState}">{connectError || connState}</span>
		{/if}
	</div>

	<div class="timeline" bind:this={scrollEl}>
		{#each timeline as item, i (i)}
			{#if item.kind === 'message'}
				<div class="message {item.role}">
					<div class="message-role">{item.role === 'user' ? 'You' : 'Claude'}</div>
					<div class="message-body">
						{#if item.role === 'assistant'}
							<Markdown text={item.text} />
						{:else}
							<p>{item.text}</p>
						{/if}
					</div>
				</div>
			{:else if item.kind === 'tool_call'}
				<ToolCallCard toolKind={item.toolKind} title={item.title} status={item.status} diff={item.diff} output={item.output} />
			{:else if item.kind === 'plan'}
				<PlanView entries={item.entries} />
			{/if}
		{:else}
			<p class="muted">Say something to get started.</p>
		{/each}

		{#each pendingPermissions as p (p.requestId)}
			<PermissionPrompt title={p.title} options={p.options} onRespond={(id) => respondPermission(p.requestId, id)} />
		{/each}
	</div>

	<form class="composer" onsubmit={(e) => { e.preventDefault(); sendMessage(); }}>
		<textarea
			bind:this={textareaEl}
			bind:value={draft}
			placeholder="Ask Claude Code about this project…"
			rows="3"
			oninput={(e) => growTextarea(e.currentTarget)}
			onkeydown={(e) => {
				if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendMessage(); }
			}}
		></textarea>
		<div class="composer-footer">
			<div class="composer-selects">
				{#if modelOption}
					<select
						class="composer-select"
						value={modelOption.current_value}
						title={modelOption.name}
						onchange={(e) => changeModel((e.target as HTMLSelectElement).value)}
					>
						{#each modelOption.options as m (m.value)}
							<option value={m.value}>{m.name}</option>
						{/each}
					</select>
				{/if}
				{#if availableModes.length}
					<select
						class="composer-select"
						value={currentMode}
						title="Permission mode"
						onchange={(e) => changeMode((e.target as HTMLSelectElement).value)}
					>
						{#each availableModes as mode (mode.id)}
							<option value={mode.id} title={mode.description ?? ''}>{mode.name}</option>
						{/each}
					</select>
				{/if}
			</div>
			<button type="submit" class="btn btn-primary" disabled={!draft.trim() || connState !== 'connected'}>Send</button>
		</div>
	</form>
</div>

<style>
	.agent-pane { display: flex; flex-direction: column; height: 100%; overflow: hidden; }
	.pane-header { min-height: 0; }
	.conn-status { display: block; font-size: 11px; color: var(--color-text-muted); padding: 0.25rem 0.75rem; border-bottom: 1px solid var(--color-border); background: var(--color-surface-2); }
	.conn-status.failed { color: var(--color-danger); }

	.timeline { flex: 1; overflow-y: auto; padding: 1rem 0.75rem; display: flex; flex-direction: column; gap: 0.75rem; }
	.muted { color: var(--color-text-muted); font-size: 13px; }

	.message { display: flex; flex-direction: column; gap: 0.15rem; }
	.message-role { font-size: 11px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.03em; }
	.message-body { font-size: 13px; line-height: 1.5; }
	.message-body p { margin: 0; }
	.message.user .message-body { color: var(--color-text); }

	.composer { display: flex; flex-direction: column; gap: 0; padding: 0.5rem 0.75rem; border-top: 1px solid var(--color-border); min-width: 0; }
	.composer textarea { width: 100%; box-sizing: border-box; resize: none; font-family: inherit; font-size: 13px; padding: 0.5rem 0.7rem; border: 1px solid var(--color-border); border-bottom: none; border-radius: 6px 6px 0 0; background: var(--color-surface); color: var(--color-text); overflow-y: auto; max-height: calc(8 * 1.5em + 1rem); line-height: 1.5; }
	.composer-footer { display: flex; align-items: center; gap: 0.5rem; padding: 0.35rem 0.5rem; border: 1px solid var(--color-border); border-top: 1px solid var(--color-border-muted, var(--color-border)); border-radius: 0 0 6px 6px; background: var(--color-surface-muted, var(--color-surface)); }
	.composer-selects { display: flex; gap: 0.4rem; flex: 1; }
	.composer-select { font-size: 11px; padding: 0.15rem 0.4rem; border-radius: 4px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; }
	.composer-select:focus { outline: none; border-color: var(--color-accent, #4f8ef7); color: var(--color-text); }
</style>
