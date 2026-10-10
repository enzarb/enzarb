export type ToastTone = 'info' | 'success' | 'warning' | 'error';

export interface ToastItem {
	id: number;
	tone: ToastTone;
	title: string;
	description?: string;
}

export interface ToastOptions {
	tone?: ToastTone;
	description?: string;
	/** Milliseconds until it dismisses itself; 0 keeps it until dismissed. */
	timeout?: number;
}

/** Queue behind the ToastRegion. Anything may call `toasts.push(...)`. */
class Toasts {
	items = $state<ToastItem[]>([]);
	#next = 1;
	#timers = new Map<number, ReturnType<typeof setTimeout>>();

	push(title: string, options: ToastOptions = {}): number {
		const { tone = 'info', description, timeout = tone === 'error' ? 10000 : 5000 } = options;
		const id = this.#next++;
		this.items.push({ id, tone, title, description });
		if (timeout > 0)
			this.#timers.set(
				id,
				setTimeout(() => this.dismiss(id), timeout)
			);
		return id;
	}

	dismiss(id: number): void {
		clearTimeout(this.#timers.get(id));
		this.#timers.delete(id);
		this.items = this.items.filter((t) => t.id !== id);
	}

	clear(): void {
		for (const id of [...this.#timers.keys()]) clearTimeout(this.#timers.get(id));
		this.#timers.clear();
		this.items = [];
	}
}

export const toasts = new Toasts();
