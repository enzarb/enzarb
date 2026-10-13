import { page } from 'vitest/browser';
import { afterEach, describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { THEME_KEY } from '#lib/theme.ts';
import ThemeToggle from './ThemeToggle.svelte';

afterEach(() => {
	localStorage.removeItem(THEME_KEY);
	document.documentElement.removeAttribute('data-theme');
});

describe('theme across tabs', () => {
	it('follows a change made in another tab (storage event)', async () => {
		render(ThemeToggle);
		await expect.element(page.getByRole('radio', { name: 'System' })).toBeChecked();
		localStorage.setItem(THEME_KEY, 'dark');
		window.dispatchEvent(new StorageEvent('storage', { key: THEME_KEY, newValue: 'dark' }));
		await expect.element(page.getByRole('radio', { name: 'Dark' })).toBeChecked();
		expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
	});
});
