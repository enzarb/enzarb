import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { ModeState } from '#lib/acp/types.ts';
import ChatModeSwitch from './ChatModeSwitch.svelte';

const mode = (over: Partial<ModeState> = {}): ModeState => ({
	modeId: 'default',
	bypassAllowed: false,
	available: null,
	...over
});
const options = () =>
	[...document.querySelectorAll('option')].map((o) => ({ value: o.value, disabled: o.disabled }));
const select = () => page.getByRole('combobox', { name: 'Mode' });
const opts = (...ids: string[]) => ids.map((id) => ({ id, name: id }));

describe('ChatModeSwitch', () => {
	it('limits the options to the advertised modes', async () => {
		render(ChatModeSwitch, {
			mode: mode({ available: opts('default', 'plan') }),
			onSelect: vi.fn()
		});
		await expect.element(select()).toBeVisible();
		expect(options().map((o) => o.value)).toEqual(['default', 'plan']);
	});

	it("lists ez's modes by name and selects any of them without a confirm", async () => {
		const onSelect = vi.fn();
		render(ChatModeSwitch, {
			mode: mode({
				modeId: 'plan',
				available: [
					{ id: 'plan', name: 'Plan', description: 'Read-only' },
					{ id: 'build', name: 'Build' }
				]
			}),
			onSelect
		});
		await expect.element(select()).toHaveValue('plan');
		expect([...document.querySelectorAll('option')].map((o) => o.textContent)).toEqual([
			'Plan',
			'Build'
		]);
		await select().selectOptions('build');
		expect(onSelect).toHaveBeenCalledWith('build');
	});

	it('omits bypass when it is not allowed and offers it when allowed', async () => {
		const r = render(ChatModeSwitch, { mode: mode(), onSelect: vi.fn() });
		await expect.element(select()).toBeVisible();
		expect(options().map((o) => o.value)).toEqual(['default', 'plan']);
		await r.rerender({ mode: mode({ bypassAllowed: true }), onSelect: vi.fn() });
		expect(options().map((o) => o.value)).toContain('bypassPermissions');
	});

	it('shows an active bypass as selected even when it cannot be chosen', async () => {
		render(ChatModeSwitch, { mode: mode({ modeId: 'bypassPermissions' }), onSelect: vi.fn() });
		await expect.element(select()).toHaveValue('bypassPermissions');
		expect(options().find((o) => o.value === 'bypassPermissions')?.disabled).toBe(true);
	});

	it('is disabled when asked to be', async () => {
		render(ChatModeSwitch, { mode: mode(), disabled: true, onSelect: vi.fn() });
		await expect.element(select()).toBeDisabled();
	});

	it('selecting plan calls onSelect with no confirm', async () => {
		const onSelect = vi.fn();
		render(ChatModeSwitch, { mode: mode(), onSelect });
		await select().selectOptions('plan');
		expect(onSelect).toHaveBeenCalledWith('plan');
		expect(document.body.textContent).not.toContain('Turn off permission prompts?');
	});

	it('bypass needs an inline confirm, and Cancel drops it', async () => {
		const onSelect = vi.fn();
		render(ChatModeSwitch, { mode: mode({ bypassAllowed: true }), onSelect });
		await select().selectOptions('bypassPermissions');
		await expect.element(page.getByText('Turn off permission prompts?')).toBeVisible();
		expect(onSelect).not.toHaveBeenCalled();
		await page.getByRole('button', { name: 'Cancel' }).click();
		expect(document.body.textContent).not.toContain('Turn off permission prompts?');
		expect(onSelect).not.toHaveBeenCalled();

		await select().selectOptions('bypassPermissions');
		await page.getByRole('button', { name: 'Confirm' }).click();
		expect(onSelect).toHaveBeenCalledExactlyOnceWith('bypassPermissions');
		expect(document.body.textContent).not.toContain('Turn off permission prompts?');
	});
});
