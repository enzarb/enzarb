import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import RepoFormHarness from './form/__fixtures__/RepoFormHarness.svelte';
import RepoListInput from './RepoListInput.svelte';

describe('RepoListInput', () => {
	it('adds and removes rows with indexed field names', async () => {
		render(RepoListInput, { initial: [{ url: 'https://github.com/a/b', defaultBranch: 'main' }] });
		await expect
			.element(page.getByRole('textbox').first())
			.toHaveAttribute('name', 'repositories[0].url');

		await page.getByRole('button', { name: 'Add repository' }).click();
		await expect
			.element(page.getByRole('textbox').nth(2))
			.toHaveAttribute('name', 'repositories[1].url');

		await page.getByRole('button', { name: 'Remove repository 1' }).click();
		await expect.element(page.getByRole('textbox').first()).toHaveValue('');
		await expect
			.element(page.getByRole('button', { name: 'Remove repository 2' }))
			.not.toBeInTheDocument();
	});

	it('shows issues for a row', async () => {
		render(RepoListInput, {
			initial: [{ url: 'nope', defaultBranch: 'main' }],
			issues: (i: number, f: string) => (i === 0 && f === 'url' ? ['bad url'] : [])
		});
		await expect.element(page.getByText('bad url')).toBeInTheDocument();
	});

	it('restores the starting rows when the form is reset', async () => {
		render(RepoFormHarness, { field: (i: number, f: string) => ({ name: `r[${i}].${f}` }) });
		await page.getByRole('button', { name: 'Add repository' }).click();
		await page.getByRole('textbox').first().fill('https://github.com/x/y');
		await page.getByRole('button', { name: 'Reset' }).click();
		await expect.element(page.getByRole('textbox').first()).toHaveValue('https://github.com/a/b');
		await expect.element(page.getByRole('textbox').nth(1)).toHaveValue('dev');
		await expect
			.element(page.getByRole('button', { name: 'Remove repository 2' }))
			.not.toBeInTheDocument();
	});
});
