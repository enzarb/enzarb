import { page, userEvent } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { PermissionOption } from '#lib/acp/types.ts';
import PermissionRequest from './PermissionRequest.svelte';

const both: PermissionOption[] = [
	{ optionId: 'a', name: 'Allow', kind: 'allow_once' },
	{ optionId: 'r', name: 'Reject', kind: 'reject_once' }
];
const base = {
	id: 'p1',
	toolCall: { toolCallId: 't', title: 'Run rm', text: '' },
	options: both,
	status: 'pending' as const,
	epoch: 3
};

describe('PermissionRequest', () => {
	it('offers exactly Allow once and Deny, with nothing focused', async () => {
		render(PermissionRequest, { ...base, onAnswer: vi.fn() });
		await expect.element(page.getByRole('button', { name: 'Allow once' })).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Deny' })).toBeVisible();
		expect(page.getByRole('button').elements()).toHaveLength(2);
		expect(page.getByRole('button').elements()).not.toContain(document.activeElement);
	});

	it('answers with the epoch it was rendered in, by mouse and keyboard', async () => {
		const onAnswer = vi.fn();
		render(PermissionRequest, { ...base, onAnswer });
		await page.getByRole('button', { name: 'Deny' }).click();
		expect(onAnswer).toHaveBeenCalledWith('p1', 'deny', 3);
		const allow = page.getByRole('button', { name: 'Allow once' }).element() as HTMLButtonElement;
		allow.focus();
		await userEvent.keyboard('{Enter}');
		expect(onAnswer).toHaveBeenCalledWith('p1', 'allow', 3);
	});

	it('shows no Allow button without an allow_once option', async () => {
		render(PermissionRequest, { ...base, options: [both[1]], onAnswer: vi.fn() });
		expect(page.getByRole('button', { name: 'Allow once' }).elements()).toHaveLength(0);
		await expect.element(page.getByRole('button', { name: 'Deny' })).toBeVisible();
	});

	it('shows no buttons once the item is cancelled', async () => {
		render(PermissionRequest, { ...base, status: 'cancelled', onAnswer: vi.fn() });
		expect(page.getByRole('button').elements()).toHaveLength(0);
		await expect.element(page.getByText('Cancelled')).toBeVisible();
	});

	it('shows a blocked item with the reason and no buttons', async () => {
		render(PermissionRequest, {
			...base,
			status: 'blocked',
			options: [],
			reason: 'no network',
			onAnswer: vi.fn()
		});
		await expect.element(page.getByText('Blocked by sandbox policy: no network')).toBeVisible();
		expect(page.getByRole('button').elements()).toHaveLength(0);
	});

	it('shows the policy hint as plain text', async () => {
		render(PermissionRequest, {
			...base,
			hint: 'Sandbox policy: would deny — <b>x</b>',
			onAnswer: vi.fn()
		});
		await expect.element(page.getByText('Sandbox policy: would deny — <b>x</b>')).toBeVisible();
		expect(document.querySelector('.ez-chat-perm b')).toBeNull();
	});
});
