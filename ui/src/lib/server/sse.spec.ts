import { describe, expect, it } from 'vitest';
import { sseStream } from './sse.ts';

describe('sseStream', () => {
	it('streams data frames with event-stream headers and cleans up on abort', async () => {
		const ctl = new AbortController();
		let send!: (d: unknown) => void;
		let cleaned = 0;
		const res = sseStream(
			ctl.signal,
			(s) => {
				send = s;
				return () => cleaned++;
			},
			10_000
		);
		expect(res.headers.get('content-type')).toBe('text/event-stream');
		const reader = res.body!.getReader();
		const dec = new TextDecoder();
		expect(dec.decode((await reader.read()).value)).toContain('retry:');
		send({ id: 5 });
		expect(dec.decode((await reader.read()).value)).toBe('data: {"id":5}\n\n');
		ctl.abort();
		expect(cleaned).toBe(1);
		await reader.cancel();
		expect(cleaned).toBe(1);
	});

	it('sends heartbeat comments', async () => {
		const ctl = new AbortController();
		const res = sseStream(ctl.signal, () => () => {}, 5);
		const reader = res.body!.getReader();
		await reader.read();
		expect(new TextDecoder().decode((await reader.read()).value)).toBe(': keep-alive\n\n');
		ctl.abort();
	});
});
