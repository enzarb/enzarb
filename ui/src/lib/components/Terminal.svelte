<script lang="ts">
	import '@xterm/xterm/css/xterm.css';
	import type { Session } from '@enzarb/proto';
	import Button from '#lib/components/form/Button.svelte';
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
		onclose,
		fill = false
	}: {
		project: string;
		/** Run this instead of the default shell. */
		command?: string[];
		/** The sandbox reported `credential_captured`. */
		oncredential?: () => void;
		/** The PTY ended; `message` is a human description of the CLOSE reason. */
		onclose?: (message: string) => void;
		/** Grow to fill the parent (a flex column with a bounded height) instead of a fixed height. */
		fill?: boolean;
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
			// The canvas cannot read CSS variables, so take the terminal colours from the tokens once.
			const css = getComputedStyle(host);
			const term = new Terminal({
				cursorBlink: true,
				fontFamily: css.getPropertyValue('--ez-mono').trim() || 'monospace',
				fontSize: 14,
				theme: {
					background: css.getPropertyValue('--ez-terminal-bg').trim() || '#000000',
					foreground: css.getPropertyValue('--ez-terminal-fg').trim() || '#e6e6ec'
				}
			});
			const fit = new FitAddon();
			term.loadAddon(fit);
			term.open(host);
			let s: Session | undefined;
			let pty: Awaited<ReturnType<Session['openPty']>> | undefined;
			const refit = () => {
				fit.fit();
				// Only tell the sandbox once the PTY exists; openPty and the post-open resize cover the rest.
				if (s && pty) s.resize(pty, term.cols, term.rows);
			};
			// Fit after layout has settled, then again whenever the region changes size.
			const frame = requestAnimationFrame(refit);
			const resize = new ResizeObserver(refit);
			resize.observe(host);
			cleanup = () => {
				cancelAnimationFrame(frame);
				resize.disconnect();
				term.dispose();
			};

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
				session = await proto.Session.connect(ws, { client: 'ui' });
				if (disposed) return void session.close();
				advance('handshake'); // connecting is already done if `open` fired
				s = session;
				s.onCtrl((m) => {
					if (m.type === 'credential_captured') oncredential?.();
					else if (m.type === 'error') term.writeln(`\r\n[error ${m.code}] ${m.message}`);
				});

				const opened = await s.openPty({ cols: term.cols, rows: term.rows, command });
				pty = opened;
				// The region may have changed size while connecting.
				s.resize(opened, term.cols, term.rows);
				advance('shell');
				term.focus();

				const input = term.onData((d) => void opened.write(d).catch(() => {}));
				const prev = cleanup;
				cleanup = () => {
					input.dispose();
					prev();
				};

				for await (const d of opened) term.write(d.data);
				if (disposed) return;
				ended = describeClose(opened.closeInfo?.reason);
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

<div class={fill ? 'ez-stack ez-stack--fill' : 'ez-stack'}>
	{#if progress.current < 4 || progress.failure}
		<TerminalChecklist {progress} />
	{/if}
	{#if progress.failure}
		<div><Button onclick={reconnect}>Try again</Button></div>
	{/if}
	{#if ended}
		<p class="ez-muted ez-cluster" role="status">
			{ended}. <Button size="sm" onclick={reconnect}>Reconnect</Button>
		</p>
	{/if}
	<div class={fill ? 'ez-terminal ez-terminal--fill' : 'ez-terminal'} bind:this={host}></div>
</div>
