import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createLiveWatcher, POLL_MS, type LiveSocket } from './live.ts';

class FakeSocket implements LiveSocket {
	static all: FakeSocket[] = [];
	onopen: LiveSocket['onopen'] = null;
	onmessage: LiveSocket['onmessage'] = null;
	onclose: LiveSocket['onclose'] = null;
	onerror: LiveSocket['onerror'] = null;
	closed = false;
	constructor(public url: string) {
		FakeSocket.all.push(this);
	}
	close() {
		this.closed = true;
	}
	open() {
		this.onopen?.({});
	}
	send(data: unknown) {
		this.onmessage?.({ data });
	}
	drop(code = 1006) {
		this.onclose?.({ code });
	}
}

const STREAM = 'wss://x/live/v1/stream';
const refresh = vi.fn(async () => ({ url: STREAM, expiresInSeconds: 300 }));

function start(random = () => 1) {
	const onChange = vi.fn();
	const stop = createLiveWatcher(
		{
			WebSocket: FakeSocket,
			refresh,
			setTimeout: (fn, ms) => setTimeout(fn, ms),
			clearTimeout: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
			random
		},
		onChange
	);
	return { onChange, stop };
}

const last = () => FakeSocket.all[FakeSocket.all.length - 1];

beforeEach(() => {
	vi.useFakeTimers();
	FakeSocket.all = [];
	refresh.mockClear();
	refresh.mockImplementation(async () => ({ url: STREAM, expiresInSeconds: 300 }));
});
afterEach(() => vi.useRealTimers());

describe('createLiveWatcher', () => {
	it('maps work_item and resync frames and ignores junk', async () => {
		const { onChange } = start();
		await vi.advanceTimersByTimeAsync(0);
		last().open();
		last().send('{"topic":"work_item","id":7,"project":"p"}');
		last().send('{"topic":"resync"}');
		last().send('not json');
		last().send('{"topic":"other","id":1}');
		last().send('{"topic":"work_item","id":"7"}');
		last().send('null');
		expect(onChange.mock.calls).toEqual([[7], [null]]);
	});

	it('does not refresh on the first open', async () => {
		const { onChange } = start();
		await vi.advanceTimersByTimeAsync(0);
		last().open();
		expect(onChange).not.toHaveBeenCalled();
	});

	it('opens a query-less URL after refreshing the cookie', async () => {
		start();
		await vi.advanceTimersByTimeAsync(0);
		expect(refresh).toHaveBeenCalledTimes(1);
		expect(last().url).toBe(STREAM);
		expect(last().url).not.toContain('token');
		expect(last().url).not.toContain('?');
	});

	it('uses the legacy URL only when refresh returns it', async () => {
		refresh.mockResolvedValueOnce({ url: `${STREAM}?token=t`, expiresInSeconds: 300 });
		start();
		await vi.advanceTimersByTimeAsync(0);
		expect(last().url).toBe(`${STREAM}?token=t`);
	});

	it('renews the cookie before it expires and stops on unsubscribe', async () => {
		const { stop } = start();
		await vi.advanceTimersByTimeAsync(0);
		last().open();
		await vi.advanceTimersByTimeAsync(239_999);
		expect(refresh).toHaveBeenCalledTimes(1);
		await vi.advanceTimersByTimeAsync(1);
		expect(refresh).toHaveBeenCalledTimes(2);
		// a timer refresh does not touch the socket
		expect(FakeSocket.all).toHaveLength(1);
		expect(last().closed).toBe(false);
		stop();
		expect(vi.getTimerCount()).toBe(0);
		await vi.advanceTimersByTimeAsync(600_000);
		expect(refresh).toHaveBeenCalledTimes(2);
	});

	it('keeps exactly one refresh timer across reconnects', async () => {
		start();
		await vi.advanceTimersByTimeAsync(0);
		last().open();
		expect(vi.getTimerCount()).toBe(1);
		for (let i = 0; i < 3; i++) {
			last().drop();
			await vi.advanceTimersByTimeAsync(POLL_MS - 1);
			expect(FakeSocket.all.length).toBeGreaterThan(i + 1);
			last().open();
			expect(vi.getTimerCount()).toBe(1);
		}
	});

	it('reconnects with backoff and refreshes first, then refreshes fully', async () => {
		const { onChange } = start();
		await vi.advanceTimersByTimeAsync(0);
		last().open();
		last().drop();
		expect(FakeSocket.all).toHaveLength(1);
		await vi.advanceTimersByTimeAsync(1_000);
		expect(FakeSocket.all).toHaveLength(2);
		expect(refresh).toHaveBeenCalledTimes(2);
		last().open();
		expect(onChange.mock.calls).toEqual([[null]]);
	});

	it('doubles the backoff up to a cap and resets on open', async () => {
		start();
		await vi.advanceTimersByTimeAsync(0);
		last().drop();
		await vi.advanceTimersByTimeAsync(1_000);
		last().drop();
		await vi.advanceTimersByTimeAsync(1_999);
		expect(FakeSocket.all).toHaveLength(2);
		await vi.advanceTimersByTimeAsync(1);
		expect(FakeSocket.all).toHaveLength(3);
		last().open();
		last().drop();
		await vi.advanceTimersByTimeAsync(1_000);
		expect(FakeSocket.all).toHaveLength(4);
	});

	it('reconnects at once on close 4001, but only once in a row', async () => {
		start();
		await vi.advanceTimersByTimeAsync(0);
		last().open();
		last().drop(4001);
		await vi.advanceTimersByTimeAsync(0);
		expect(FakeSocket.all).toHaveLength(2);
		expect(refresh).toHaveBeenCalledTimes(2);
		last().drop(4001);
		await vi.advanceTimersByTimeAsync(0);
		expect(FakeSocket.all).toHaveLength(2);
		await vi.advanceTimersByTimeAsync(1_000);
		expect(FakeSocket.all).toHaveLength(3);
	});

	it('retries with backoff when the token fetch fails and refreshes on open', async () => {
		refresh.mockRejectedValueOnce(new Error('401'));
		const { onChange } = start();
		await vi.advanceTimersByTimeAsync(0);
		expect(FakeSocket.all).toHaveLength(0);
		await vi.advanceTimersByTimeAsync(1_000);
		expect(FakeSocket.all).toHaveLength(1);
		last().open();
		expect(onChange.mock.calls).toEqual([[null]]);
	});

	it('polls every minute while disconnected only', async () => {
		const { onChange } = start();
		await vi.advanceTimersByTimeAsync(0);
		last().open();
		await vi.advanceTimersByTimeAsync(POLL_MS * 2);
		expect(onChange).not.toHaveBeenCalled();
		last().drop();
		refresh.mockImplementation(() => new Promise(() => {}));
		await vi.advanceTimersByTimeAsync(POLL_MS);
		expect(onChange).toHaveBeenCalledTimes(1);
		await vi.advanceTimersByTimeAsync(POLL_MS);
		expect(onChange).toHaveBeenCalledTimes(2);
	});

	it('stops polling once reconnected', async () => {
		const { onChange } = start();
		await vi.advanceTimersByTimeAsync(0);
		last().drop();
		await vi.advanceTimersByTimeAsync(1_000);
		last().open();
		onChange.mockClear();
		await vi.advanceTimersByTimeAsync(POLL_MS * 2);
		expect(onChange).not.toHaveBeenCalled();
	});

	it('unsubscribe closes the socket and stops reconnects and timers', async () => {
		const { onChange, stop } = start();
		await vi.advanceTimersByTimeAsync(0);
		last().open();
		const ws = last();
		ws.drop();
		stop();
		await vi.advanceTimersByTimeAsync(POLL_MS * 3);
		expect(FakeSocket.all).toHaveLength(1);
		expect(onChange).not.toHaveBeenCalled();
		const second = start();
		await vi.advanceTimersByTimeAsync(0);
		last().open();
		second.stop();
		expect(last().closed).toBe(true);
		last().drop();
		await vi.advanceTimersByTimeAsync(POLL_MS);
		expect(FakeSocket.all).toHaveLength(2);
	});
});
