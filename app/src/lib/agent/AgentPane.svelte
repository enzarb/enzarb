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
	import {
		notificationsEnabled,
		notificationsSupported,
		setNotificationsEnabled,
		notify,
		looksLikeQuestion
	} from '$lib/agent/notifications';

	type TimelineItem =
		| { kind: 'message'; role: 'user' | 'assistant'; text: string }
		| { kind: 'thought'; text: string }
		| { kind: 'tool_call'; id: string; toolKind: string; title: string; status: string; path: string | null; diff: DiffPayload | null; output: string | null; plan: string | null }
		| { kind: 'plan'; entries: PlanEntryPayload[] };

	type PendingPermission = {
		requestId: string;
		toolCallId: string;
		title: string;
		options: PermissionOptionPayload[];
		plan: string | null;
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
	// The server resends full event history on every (re)connect as a burst of
	// individual events; scrolling on each one makes the whole conversation
	// visibly replay/scroll past. Suppress scrolling until the burst quiets
	// down, then scroll once — after that, live events scroll immediately.
	let historySettled = false;
	let historyTimer: ReturnType<typeof setTimeout> | undefined;
	let textareaEl: HTMLTextAreaElement | undefined = $state();
	let availableModes: SessionModeInfo[] = $state([]);
	let currentMode: string = $state('default');
	let configOptions: ConfigOptionInfo[] = $state([]);
	// The backend can report more than one config option for the same
	// conceptual setting (e.g. two entries both categorized "model"); keep
	// only the first per category so the composer doesn't show duplicate
	// pickers for the same thing.
	const visibleConfigOptions = $derived(
		Array.from(
			new Map(configOptions.map((o) => [o.category ?? o.id, o])).values()
		)
	);
	let running = $state(false);
	let notifyEnabled = $state(false);
	// Set in onMount so SSR and hydration render the same markup.
	let notifySupported = $state(false);

	async function toggleNotifications() {
		notifyEnabled = await setNotificationsEnabled(!notifyEnabled);
	}

	// Fire a browser notification when the agent is waiting on the user:
	// explicitly for permission requests, heuristically when the turn ends on
	// question-like assistant text. historySettled gates out the event-history
	// replay bursts on (re)connect.
	function notifyIfQuestion() {
		const lastAssistant = [...timeline]
			.reverse()
			.find((t) => t.kind === 'message' && t.role === 'assistant');
		if (lastAssistant?.kind === 'message' && looksLikeQuestion(lastAssistant.text)) {
			notify(`Claude asked a question — ${project}`, lastAssistant.text.trim(), `agent-q-${sessionId}`);
		}
	}

	function changeConfigOption(option: ConfigOptionInfo, value: string) {
		if (value === option.current_value) return;
		configOptions = configOptions.map((o) =>
			o.id === option.id ? { ...o, current_value: value } : o
		);
		socket?.send({ type: 'set_config_option', config_id: option.id, value });
	}

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
			case 'thought_chunk': {
				const last = timeline[timeline.length - 1];
				if (last?.kind === 'thought') last.text += event.text;
				else timeline.push({ kind: 'thought', text: event.text });
				break;
			}
			case 'tool_call_created':
				timeline.push({
					kind: 'tool_call',
					id: event.tool_call_id,
					toolKind: event.kind,
					title: event.title,
					status: event.status,
					path: event.path,
					diff: null,
					output: null,
					plan: event.plan
				});
				break;
			case 'tool_call_updated': {
				const item = [...timeline].reverse().find((t) => t.kind === 'tool_call' && t.id === event.tool_call_id);
				if (item && item.kind === 'tool_call') {
					if (event.status) item.status = event.status;
					if (event.diff) item.diff = event.diff;
					if (event.output) item.output = event.output;
					if (event.path) item.path = event.path;
					if (event.plan) item.plan = event.plan;
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
				if (historySettled) {
					notify(`Claude needs permission — ${project}`, event.title, `agent-perm-${sessionId}`);
				}
				pendingPermissions.push({
					requestId: event.request_id,
					toolCallId: event.tool_call_id,
					title: event.title,
					options: event.options,
					plan: event.plan
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
			case 'turn_status': {
				const wasRunning = running;
				running = event.running;
				if (wasRunning && !running) {
					if (queuedMessages.length && socket) {
						const next = queuedMessages.shift();
						if (next) socket.send({ type: 'send_message', text: next });
					} else if (historySettled) {
						notifyIfQuestion();
					}
				}
				break;
			}
		}
		scheduleScroll();
	}

	function scrollToBottom() {
		queueMicrotask(() => scrollEl?.scrollTo({ top: scrollEl.scrollHeight }));
	}

	function scheduleScroll() {
		if (historySettled) {
			scrollToBottom();
			return;
		}
		clearTimeout(historyTimer);
		historyTimer = setTimeout(() => {
			historySettled = true;
			scrollToBottom();
		}, 80);
	}

	function respondPermission(requestId: string, optionId: string) {
		// The socket can report 'connected' and still momentarily reject a send
		// (e.g. mid-reconnect); surface that instead of silently dropping the
		// response and leaving the prompt looking stuck.
		const sent = socket?.send({ type: 'permission_response', request_id: requestId, option_id: optionId });
		if (!sent) connectError = "Couldn't send response — reconnecting, please try again once connected.";
	}

	// Messages sent while a turn is running can't go to the agent immediately —
	// only one prompt runs at a time — so they're echoed right away and queued
	// to dispatch as soon as the current turn finishes.
	let queuedMessages: string[] = [];

	function sendMessage() {
		const text = draft.trim();
		if (!text || !socket) return;
		timeline.push({ kind: 'message', role: 'user', text });
		draft = '';
		if (textareaEl) { textareaEl.style.height = 'auto'; }
		scrollToBottom();
		if (running) {
			queuedMessages.push(text);
		} else {
			socket.send({ type: 'send_message', text });
		}
	}

	function stopTurn() {
		socket?.send({ type: 'cancel' });
	}

	function growTextarea(el: HTMLTextAreaElement) {
		el.style.height = 'auto';
		el.style.height = el.scrollHeight + 'px';
	}

	onMount(async () => {
		mounted = true;
		notifySupported = notificationsSupported();
		notifyEnabled = notificationsEnabled();
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
				// turn_status isn't part of that history, so reset it too —
				// worst case the user has to press send again.
				timeline = [];
				pendingPermissions = [];
				running = false;
				queuedMessages = [];
				historySettled = false;
				clearTimeout(historyTimer);
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
			{:else if item.kind === 'thought'}
				<details class="thought">
					<summary>Thinking…</summary>
					<Markdown text={item.text} />
				</details>
			{:else if item.kind === 'tool_call'}
				<ToolCallCard toolKind={item.toolKind} title={item.title} status={item.status} path={item.path} diff={item.diff} output={item.output} plan={item.plan} />
			{:else if item.kind === 'plan'}
				<PlanView entries={item.entries} />
			{/if}
		{:else}
			<p class="muted">Say something to get started.</p>
		{/each}

		{#each pendingPermissions as p (p.requestId)}
			<PermissionPrompt title={p.title} options={p.options} plan={p.plan} disabled={connState !== 'connected'} onRespond={(id) => respondPermission(p.requestId, id)} />
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
				{#each visibleConfigOptions as option (option.id)}
					<select
						class="composer-select"
						value={option.current_value}
						title={option.name}
						onchange={(e) => changeConfigOption(option, (e.target as HTMLSelectElement).value)}
					>
						{#each option.options as v (v.value)}
							<option value={v.value}>{v.name}</option>
						{/each}
					</select>
				{/each}
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
			<div class="composer-buttons">
				{#if notifySupported}
					<button
						type="button"
						class="notify-toggle"
						class:on={notifyEnabled}
						title={notifyEnabled
							? 'Notifications on — click to disable'
							: 'Notify me when Claude asks a question'}
						onclick={toggleNotifications}
					>{notifyEnabled ? '🔔' : '🔕'}</button>
				{/if}
				<button type="button" class="btn btn-danger" onclick={stopTurn} disabled={!running || connState !== 'connected'}>Stop</button>
				<button type="submit" class="btn btn-primary" disabled={!draft.trim() || connState !== 'connected'}>Send</button>
			</div>
		</div>
	</form>
</div>

<style>
	.agent-pane { display: flex; flex-direction: column; height: 100%; overflow: hidden; }
	.pane-header { min-height: 0; }
	.notify-toggle { background: none; border: none; cursor: pointer; font-size: 13px; padding: 0.15rem 0.3rem; opacity: 0.45; line-height: 1; }
	.notify-toggle:hover, .notify-toggle.on { opacity: 1; }
	.conn-status { display: block; font-size: 11px; color: var(--color-text-muted); padding: 0.25rem 0.75rem; border-bottom: 1px solid var(--color-border); background: var(--color-surface-2); }
	.conn-status.failed { color: var(--color-danger); }

	.timeline {
		flex: 1;
		overflow-y: auto;
		overflow-x: hidden;
		min-width: 0;
		padding: 1rem 0.75rem;
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
		scrollbar-width: thin;
		scrollbar-color: var(--color-border) transparent;
	}
	.timeline::-webkit-scrollbar { width: 8px; height: 0; }
	.timeline::-webkit-scrollbar-track { background: transparent; }
	.timeline::-webkit-scrollbar-thumb { background: var(--color-border); border-radius: 4px; }
	.timeline::-webkit-scrollbar-thumb:hover { background: var(--color-text-muted); }
	.muted { color: var(--color-text-muted); font-size: 13px; }

	.message { display: flex; flex-direction: column; gap: 0.15rem; min-width: 0; }
	.message-role { font-size: 11px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.03em; }
	.message-body { font-size: 13px; line-height: 1.5; min-width: 0; overflow-wrap: anywhere; }
	.message-body p { margin: 0; overflow-wrap: anywhere; white-space: pre-wrap; }
	.message.user .message-body { color: var(--color-text); }

	.thought { font-size: 12px; color: var(--color-text-muted); font-style: italic; }
	.thought summary { cursor: pointer; user-select: none; }
	.thought summary:hover { color: var(--color-text); }
	.thought :global(p) { margin: 0.3rem 0 0; overflow-wrap: anywhere; white-space: pre-wrap; }

	.composer { display: flex; flex-direction: column; gap: 0; padding: 0.5rem 0.75rem; border-top: 1px solid var(--color-border); min-width: 0; }
	.composer textarea { width: 100%; box-sizing: border-box; resize: none; font-family: inherit; font-size: 13px; padding: 0.5rem 0.7rem; border: 1px solid var(--color-border); border-bottom: none; border-radius: 6px 6px 0 0; background: var(--color-surface); color: var(--color-text); overflow-y: auto; max-height: calc(8 * 1.5em + 1rem); line-height: 1.5; }
	.composer-footer { display: flex; align-items: center; gap: 0.5rem; padding: 0.35rem 0.5rem; border: 1px solid var(--color-border); border-top: 1px solid var(--color-border-muted, var(--color-border)); border-radius: 0 0 6px 6px; background: var(--color-surface-muted, var(--color-surface)); }
	.composer-selects { display: flex; gap: 0.4rem; flex: 1; }
	.composer-buttons { display: flex; gap: 0.4rem; flex-shrink: 0; }
	.composer-select { font-size: 11px; padding: 0.15rem 0.4rem; border-radius: 4px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; }
	.composer-select:focus { outline: none; border-color: var(--color-accent, #4f8ef7); color: var(--color-text); }
</style>
