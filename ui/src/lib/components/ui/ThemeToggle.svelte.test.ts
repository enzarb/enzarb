import { page, userEvent } from 'vitest/browser';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { theme } from '#lib/theme.svelte.ts';
import { THEME_KEY } from '#lib/theme.ts';
import ThemeToggle from './ThemeToggle.svelte';

const reset = () => {
	localStorage.removeItem(THEME_KEY);
	document.documentElement.removeAttribute('data-theme');
	theme.choice = 'system';
};
beforeEach(reset);
afterEach(reset);

describe('ThemeToggle', () => {
	it('defaults to following the system', async () => {
		render(ThemeToggle);
		await expect.element(page.getByRole('radio', { name: 'System' })).toBeChecked();
		expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
	});

	it('applies and remembers a choice', async () => {
		render(ThemeToggle);
		await page.getByRole('radio', { name: 'Dark' }).click();
		expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
		expect(localStorage.getItem(THEME_KEY)).toBe('dark');
		await page.getByRole('radio', { name: 'System' }).click();
		expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
		expect(localStorage.getItem(THEME_KEY)).toBeNull();
	});

	it('restores the remembered choice on the next visit', async () => {
		localStorage.setItem(THEME_KEY, 'light');
		render(ThemeToggle);
		await expect.element(page.getByRole('radio', { name: 'Light' })).toBeChecked();
	});

	it('moves with arrow keys', async () => {
		render(ThemeToggle);
		page.getByRole('radio', { name: 'System' }).element().focus();
		await userEvent.keyboard('{ArrowRight}');
		await expect.element(page.getByRole('radio', { name: 'Light' })).toBeChecked();
		expect(document.documentElement.getAttribute('data-theme')).toBe('light');
	});
});
