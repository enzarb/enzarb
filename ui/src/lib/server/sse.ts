const HEARTBEAT_MS = 25_000;

/**
 * A `text/event-stream` response. `start` registers the data source and returns its cleanup;
 * a comment heartbeat keeps proxies from closing idle streams. Everything is torn down when the
 * client disconnects (`signal`) or the stream is cancelled.
 */
export function sseStream(
	signal: AbortSignal,
	start: (send: (data: unknown) => void) => () => void,
	heartbeatMs = HEARTBEAT_MS
): Response {
	const enc = new TextEncoder();
	let cleanup: (() => void) | undefined;
	let timer: ReturnType<typeof setInterval> | undefined;
	let closed = false;

	const stop = () => {
		if (closed) return;
		closed = true;
		clearInterval(timer);
		cleanup?.();
	};

	const body = new ReadableStream<Uint8Array>({
		start(controller) {
			const write = (chunk: string) => {
				if (closed) return;
				try {
					controller.enqueue(enc.encode(chunk));
				} catch {
					stop();
				}
			};
			write('retry: 3000\n\n');
			cleanup = start((data) => write(`data: ${JSON.stringify(data)}\n\n`));
			timer = setInterval(() => write(': keep-alive\n\n'), heartbeatMs);
			if (signal.aborted) stop();
			else signal.addEventListener('abort', stop, { once: true });
		},
		cancel: stop
	});

	return new Response(body, {
		headers: {
			'content-type': 'text/event-stream',
			'cache-control': 'no-cache, no-transform',
			connection: 'keep-alive',
			'x-accel-buffering': 'no'
		}
	});
}
