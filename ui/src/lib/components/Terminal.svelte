<script lang="ts">
	import '@xterm/xterm/css/xterm.css';
	import type { Session, WebSocketLike } from '@enzarb/proto';
	import TerminalChecklist from '#lib/components/TerminalChecklist.svelte';
	import { terminalConnection } from '#lib/terminal.remote.ts';
	import {
		completeStep,
		describeClose,
		failCurrent,
		initialProgress,
		type TerminalProgress
	} from '#lib/terminal-steps.ts';

	let {
		project,
		command,
		oncredential,
		onclose
	}: {
		project: string;
		/** Run this instead of the default shell. */
		command?: string[];
		/** The sandbox reported `credential_captured`. */
		oncredential?: () => void;
		/** The PTY ended; `message` is a human description of the CLOSE reason. */
		onclose?: (message: string) => void;
	} = $props();

	let host: HTMLDivElement;
	let progress = $state<TerminalProgress>(initialProgress);
	let ended = $state('');
	let attempt = $state(0);

	const advance = (id: Parameters<typeof completeStep>[1]) =>
		(progress = completeStep(progress, id));
	const fail = (e: unknown) =>
		(progress = failCurrent(progress, e instanceof Error ? e.message : String(e)));

	function reconnect() {
		progress = initialProgress;
		ended = '';
		attempt += 1;
	}

	// Runs in the browser only; re-runs the whole connection when `attempt` changes.
	$effect(() => {
		void attempt;
		let disposed = false;
		let session: Session | undefined;
		let cleanup = () => {};

		(async () => {
			const [{ Terminal }, { FitAddon }, proto] = await Promise.all([
				import('@xterm/xterm'),
				import('@xterm/addon-fit'),
				import('@enzarb/proto')
			]);
			if (disposed) return;
			const term = new Terminal({ cursorBlink: true, fontFamily: 'monospace', fontSize: 14 });
			const fit = new FitAddon();
			term.loadAddon(fit);
			term.open(host);
			fit.fit();
			cleanup = () => term.dispose();

			let url: string;
			try {
				({ url } = await terminalConnection(project));
			} catch (e) {
				return fail(e);
			}
			if (disposed) return;
			advance('token');

			try {
				// Own the socket so "connecting" (open) and "handshake" (welcome) are separate steps.
				const ws = new WebSocket(url);
				ws.addEventListener('open', () => advance('connecting'));
				session = await proto.Session.connect(ws as unknown as WebSocketLike, { client: 'ui' });
				if (disposed) return void session.close();
				advance('handshake'); // connecting is already done if `open` fired
				const s = session;
				s.onCtrl((m) => {
					if (m.type === 'credential_captured') oncredential?.();
					else if (m.type === 'error') term.writeln(`\r\n[error ${m.code}] ${m.message}`);
				});

				const pty = await s.openPty({ cols: term.cols, rows: term.rows, command });
				advance('shell');
				term.focus();

				const input = term.onData((d) => void pty.write(d).catch(() => {}));
				const resize = new ResizeObserver(() => {
					fit.fit();
					s.resize(pty, term.cols, term.rows);
				});
				resize.observe(host);
				const prev = cleanup;
				cleanup = () => {
					input.dispose();
					resize.disconnect();
					prev();
				};

				for await (const d of pty) term.write(d.data);
				if (disposed) return;
				ended = describeClose(pty.closeInfo?.reason);
				onclose?.(ended);
			} catch (e) {
				if (!disposed) fail(e);
			}
		})();

		return () => {
			disposed = true;
			void session?.close();
			cleanup();
		};
	});
</script>

<div class="stack">
	{#if progress.current < 4 || progress.failure}
		<TerminalChecklist {progress} />
	{/if}
	{#if progress.failure}
		<p><button onclick={reconnect}>Try again</button></p>
	{/if}
	{#if ended}
		<p class="muted" role="status">{ended}. <button onclick={reconnect}>Reconnect</button></p>
	{/if}
	<div class="terminal" bind:this={host}></div>
</div>
