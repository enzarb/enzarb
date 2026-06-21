import { writable } from 'svelte/store';

export interface ConfirmOptions {
	title: string;
	message?: string;
	confirmText?: string;
	cancelText?: string;
	// Style the confirm button as a destructive action.
	danger?: boolean;
	// When set, the user must type this exact string before confirming is enabled.
	requireText?: string;
}

interface ConfirmState extends ConfirmOptions {
	open: boolean;
	resolve?: (value: boolean) => void;
}

export const confirmStore = writable<ConfirmState>({ open: false, title: '' });

// confirm opens the global confirmation dialog and resolves to the user's
// choice. Drop-in replacement for the native window.confirm, but awaitable and
// styled. Requires <ConfirmDialog /> mounted once near the app root.
export function confirm(options: ConfirmOptions): Promise<boolean> {
	return new Promise((resolve) => {
		confirmStore.set({ ...options, open: true, resolve });
	});
}

// settle resolves the pending promise (if any) and closes the dialog. Idempotent
// — a second call (e.g. from the dialog's close event) is a no-op.
export function settle(value: boolean): void {
	confirmStore.update((s) => {
		s.resolve?.(value);
		return { open: false, title: '' };
	});
}
