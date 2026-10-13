import { page, userEvent } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { MAX_PROMPT_BYTES } from '#lib/acp/types.ts';
import Composer from './Composer.svelte';

describe('Composer', () => {
	it('Enter sends and clears; Shift+Enter inserts a newline', async () => {
		const onSend = vi.fn();
		render(Composer, { onSend, onStop: vi.fn() });
		const box = page.getByRole('textbox', { name: 'Message' });
		await box.fill('line one');
		await userEvent.keyboard('{Shift>}{Enter}{/Shift}line two');
		expect(onSend).not.toHaveBeenCalled();
		await userEvent.keyboard('{Enter}');
		expect(onSend).toHaveBeenCalledWith('line one\nline two');
		await expect.element(box).toHaveValue('');
	});

	it('does not send an empty message', async () => {
		const onSend = vi.fn();
		render(Composer, { onSend, onStop: vi.fn() });
		await expect.element(page.getByRole('button', { name: 'Send' })).toBeDisabled();
		await page.getByRole('textbox', { name: 'Message' }).fill('   ');
		await userEvent.keyboard('{Enter}');
		expect(onSend).not.toHaveBeenCalled();
	});

	it('shows Stop only while a turn is running', async () => {
		const onStop = vi.fn();
		const view = render(Composer, { onSend: vi.fn(), onStop, busy: false });
		expect(page.getByRole('button', { name: 'Stop' }).elements()).toHaveLength(0);
		await view.rerender({ onSend: vi.fn(), onStop, busy: true });
		await page.getByRole('button', { name: 'Stop' }).click();
		expect(onStop).toHaveBeenCalledOnce();
		await expect.element(page.getByRole('button', { name: 'Send' })).toBeDisabled();
	});

	it('disables Send and explains when the prompt is over the byte cap', async () => {
		const onSend = vi.fn();
		render(Composer, { onSend, onStop: vi.fn() });
		const box = page.getByRole('textbox', { name: 'Message' });
		// 3 bytes each: under the cap in characters, over it in bytes
		await box.fill('€'.repeat(MAX_PROMPT_BYTES / 3 + 1));
		await expect.element(page.getByRole('alert')).toHaveTextContent('too long');
		await expect.element(page.getByRole('button', { name: 'Send' })).toBeDisabled();
		await page.getByRole('textbox', { name: 'Message' }).click();
		await userEvent.keyboard('{Enter}');
		expect(onSend).not.toHaveBeenCalled();
	});

	it('allows a prompt exactly at the cap', async () => {
		render(Composer, { onSend: vi.fn(), onStop: vi.fn() });
		await page.getByRole('textbox', { name: 'Message' }).fill('a'.repeat(MAX_PROMPT_BYTES));
		await expect.element(page.getByRole('button', { name: 'Send' })).toBeEnabled();
		expect(page.getByRole('alert').elements()).toHaveLength(0);
	});
});
