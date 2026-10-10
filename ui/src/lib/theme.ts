/** Theme choice: follow the OS (`system`) or force `light` / `dark`. Remembered in localStorage. */
export type ThemeChoice = 'system' | 'light' | 'dark';

export const THEME_KEY = 'enzarb-theme';
export const THEME_CHOICES: readonly ThemeChoice[] = ['system', 'light', 'dark'];

export const isThemeChoice = (v: unknown): v is ThemeChoice =>
	typeof v === 'string' && (THEME_CHOICES as readonly string[]).includes(v);

/** localStorage can be missing or throw (private windows, blocked storage); never let that break the page. */
function storage(): Storage | undefined {
	try {
		return globalThis.localStorage;
	} catch {
		return undefined;
	}
}

export function readTheme(store: Storage | undefined = storage()): ThemeChoice {
	try {
		const v = store?.getItem(THEME_KEY);
		return isThemeChoice(v) ? v : 'system';
	} catch {
		return 'system';
	}
}

export function writeTheme(choice: ThemeChoice, store: Storage | undefined = storage()): void {
	try {
		if (choice === 'system') store?.removeItem(THEME_KEY);
		else store?.setItem(THEME_KEY, choice);
	} catch {
		// the choice still applies for this page view
	}
}

/** `data-theme` on <html> forces a theme; without it the stylesheet follows prefers-color-scheme. */
export function applyTheme(
	choice: ThemeChoice,
	root: HTMLElement = document.documentElement
): void {
	if (choice === 'system') root.removeAttribute('data-theme');
	else root.setAttribute('data-theme', choice);
}
