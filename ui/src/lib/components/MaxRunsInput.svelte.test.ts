import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import MaxRunsInput from './MaxRunsInput.svelte';

const input = () => page.getByRole('spinbutton', { name: 'Concurrent runs' });

describe('MaxRunsInput', () => {
	it('shows 2 as the default with a hint', async () => {
		render(MaxRunsInput, {});
		await expect.element(input()).toHaveValue(2);
		await expect.element(page.getByText('Default: 2')).toBeInTheDocument();
		await expect.element(input()).not.toHaveAttribute('aria-invalid');
	});

	for (const bad of ['0', '17', '2.5']) {
		it(`flags ${bad} inline`, async () => {
			render(MaxRunsInput, {});
			await input().fill(bad);
			await expect.element(input()).toHaveAttribute('aria-invalid', 'true');
		});
	}

	it('accepts the bounds without a message', async () => {
		render(MaxRunsInput, {});
		for (const ok of ['1', '16']) {
			await input().fill(ok);
			await expect.element(input()).not.toHaveAttribute('aria-invalid');
		}
	});

	it('treats empty input as an error unless allowEmpty', async () => {
		const strict = render(MaxRunsInput, {});
		await input().fill('');
		await expect.element(input()).toHaveAttribute('aria-invalid', 'true');
		strict.unmount();
		render(MaxRunsInput, { allowEmpty: true });
		await input().fill('');
		await expect.element(input()).not.toHaveAttribute('aria-invalid');
	});
});
