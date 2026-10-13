import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const h = vi.hoisted(() => ({
	fit: vi.fn(),
	resize: vi.fn(),
	pty: { write: vi.fn(async () => {}), closeInfo: undefined as unknown },
	releasePty: undefined as undefined | (() => void),
	openPty: undefined as undefined | (() => Promise<unknown>),
	rafCbs: [] as FrameRequestCallback[],
	observers: [] as { cb: () => void; disconnect: ReturnType<typeof vi.fn> }[]
}));

vi.mock('@xterm/xterm/css/xterm.css', () => ({}));
vi.mock('@xterm/xterm', () => ({
	Terminal: class {
		cols = 100;
		rows = 30;
		loadAddon() {}
		open() {}
		focus() {}
		write() {}
		writeln() {}
		onData() {
			return { dispose() {} };
		}
		dispose() {}
	}
}));
vi.mock('@xterm/addon-fit', () => ({
	FitAddon: class {
		fit = h.fit;
	}
}));
vi.mock('#lib/terminal.remote.ts', () => ({
	terminalConnection: async () => ({ url: 'ws://localhost/x' })
}));
vi.mock('@enzarb/proto', () => ({
	Session: {
		connect: async () => ({
			onCtrl() {},
			close: async () => {},
			resize: h.resize,
			openPty: async () => {
				await new Promise<void>((r) => (h.releasePty = r));
				const pty = h.pty as unknown as AsyncIterable<{ data: string }>;
				return Object.assign(pty, {
					[Symbol.asyncIterator]: () => ({ next: () => new Promise<never>(() => {}) })
				});
			}
		})
	}
}));

import Terminal from './Terminal.svelte';

describe('Terminal fit', () => {
	beforeEach(() => {
		h.fit.mockClear();
		h.resize.mockClear();
		h.rafCbs.length = 0;
		h.observers.length = 0;
		h.releasePty = undefined;
		vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => h.rafCbs.push(cb));
		vi.stubGlobal('cancelAnimationFrame', () => {});
		vi.stubGlobal(
			'ResizeObserver',
			class {
				disconnect = vi.fn();
				constructor(cb: () => void) {
					h.observers.push({ cb, disconnect: this.disconnect });
				}
				observe() {}
			}
		);
		vi.stubGlobal(
			'WebSocket',
			class {
				addEventListener() {}
			}
		);
	});

	it('fits only after the animation frame, and on every observer callback', async () => {
		render(Terminal, { project: 'alpha', fill: true });
		await vi.waitFor(() => expect(h.observers.length).toBe(1));
		expect(h.fit).not.toHaveBeenCalled();
		h.rafCbs[0](0);
		expect(h.fit).toHaveBeenCalledTimes(1);
		// No PTY yet: fit locally, send nothing.
		h.observers[0].cb();
		expect(h.fit).toHaveBeenCalledTimes(2);
		expect(h.resize).not.toHaveBeenCalled();
	});

	it('sends the new size once the PTY is open', async () => {
		render(Terminal, { project: 'alpha', fill: true });
		await vi.waitFor(() => expect(h.releasePty).toBeDefined());
		h.releasePty!();
		// One resize right after open, in case the region changed while connecting.
		await vi.waitFor(() => expect(h.resize).toHaveBeenCalledWith(expect.anything(), 100, 30));
		h.resize.mockClear();
		h.observers[0].cb();
		expect(h.fit).toHaveBeenCalled();
		expect(h.resize).toHaveBeenCalledWith(expect.anything(), 100, 30);
	});

	it('disconnects the observer on unmount', async () => {
		const { unmount } = render(Terminal, { project: 'alpha' });
		await vi.waitFor(() => expect(h.observers.length).toBe(1));
		unmount();
		expect(h.observers[0].disconnect).toHaveBeenCalled();
	});
});
