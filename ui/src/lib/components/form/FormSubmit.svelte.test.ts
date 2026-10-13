import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import FormHarness from './__fixtures__/FormHarness.svelte';

async function posted() {
	await page.getByRole('button', { name: 'Send' }).click();
	const text = page.getByTestId('posted').element().textContent!;
	return Object.fromEntries(
		(JSON.parse(text) as [string, string][]).reduce((m, [k, v]) => {
			m.set(k, [...(m.get(k) ?? []), v]);
			return m;
		}, new Map<string, string[]>())
	);
}

describe('form submission', () => {
	it('posts every control under its name', async () => {
		render(FormHarness);
		expect(await posted()).toEqual({
			name: ['abc'],
			notes: ['hi'],
			fruit: ['pear'],
			repos: ['apple', 'pear'],
			issues: ['on'],
			live: ['on'],
			size: ['apple']
		});
	});

	it('reflects changes and omits unchecked boxes', async () => {
		render(FormHarness);
		await page.getByRole('textbox', { name: 'Name' }).fill('xyz');
		await page.getByRole('checkbox', { name: 'Issues' }).click();
		await page.getByRole('checkbox', { name: 'Mentions' }).click();
		await page.getByRole('switch', { name: 'Live' }).click();
		await page.getByRole('button', { name: 'Fruit' }).click();
		await page.getByRole('option', { name: 'Apple' }).click();
		await page.getByRole('radio', { name: 'Pear' }).click();
		await page.getByRole('button', { name: 'Remove Apple' }).click();
		expect(await posted()).toEqual({
			name: ['xyz'],
			notes: ['hi'],
			fruit: ['apple'],
			repos: ['pear'],
			mentions: ['on'],
			size: ['pear']
		});
	});

	it('renders validation issues on the control', async () => {
		render(FormHarness);
		await expect.element(page.getByText('too short')).toBeInTheDocument();
		await expect
			.element(page.getByRole('textbox', { name: 'Name' }))
			.toHaveAttribute('aria-invalid', 'true');
	});
});
