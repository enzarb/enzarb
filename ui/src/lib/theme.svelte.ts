import { THEME_KEY, applyTheme, readTheme, writeTheme, type ThemeChoice } from './theme.ts';

/** Reactive theme choice shared by every toggle. Call `sync()` once in the browser after mount. */
class ThemeState {
	choice = $state<ThemeChoice>('system');

	/** Pick up what the inline script in app.html already applied before first paint. */
	sync(): void {
		this.choice = readTheme();
		if (this.#listening || typeof window === 'undefined') return;
		this.#listening = true;
		// A change made in another tab arrives as a storage event.
		window.addEventListener('storage', (e) => {
			if (e.key !== null && e.key !== THEME_KEY) return;
			this.choice = readTheme();
			applyTheme(this.choice);
		});
	}
	#listening = false;

	set(choice: ThemeChoice): void {
		this.choice = choice;
		writeTheme(choice);
		applyTheme(choice);
	}
}

export const theme = new ThemeState();
