const DISMISSED_KEY = 'enzarb-install-dismissed';

interface InstallEvent extends Event {
	prompt(): Promise<unknown>;
}

/**
 * The browser's "add to home screen" prompt, offered once from Settings. `beforeinstallprompt`
 * only fires on engines that support it; a dismissal is remembered so the button never nags.
 */
export class InstallPrompt {
	#event = $state<InstallEvent | null>(null);
	#dismissed = $state(false);

	/** Whether to offer the button: a prompt is available and the user has not dismissed it. */
	available = $derived(this.#event !== null && !this.#dismissed);

	/** Starts listening; returns the cleanup. */
	start(storage: Pick<Storage, 'getItem' | 'setItem'> = localStorage): () => void {
		try {
			this.#dismissed = storage.getItem(DISMISSED_KEY) === '1';
		} catch {
			/* storage blocked: keep showing */
		}
		const onPrompt = (e: Event) => {
			e.preventDefault();
			this.#event = e as InstallEvent;
		};
		const onInstalled = () => (this.#event = null);
		window.addEventListener('beforeinstallprompt', onPrompt);
		window.addEventListener('appinstalled', onInstalled);
		return () => {
			window.removeEventListener('beforeinstallprompt', onPrompt);
			window.removeEventListener('appinstalled', onInstalled);
		};
	}

	async install(): Promise<void> {
		const e = this.#event;
		this.#event = null;
		await e?.prompt();
	}

	dismiss(storage: Pick<Storage, 'getItem' | 'setItem'> = localStorage): void {
		this.#dismissed = true;
		try {
			storage.setItem(DISMISSED_KEY, '1');
		} catch {
			/* not persisted */
		}
	}
}
