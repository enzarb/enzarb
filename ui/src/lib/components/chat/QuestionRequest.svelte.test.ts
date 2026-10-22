import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { QuestionView } from '#lib/acp/types.ts';
import QuestionRequest from './QuestionRequest.svelte';

const questions: QuestionView[] = [
	{
		question: 'Which DB?',
		header: 'DB',
		multiSelect: false,
		options: [
			{ label: 'sqlite', description: 'one file' },
			{ label: 'postgres', description: '' }
		]
	},
	{
		question: 'Extras?',
		header: '',
		multiSelect: true,
		options: [
			{ label: 'a', description: '' },
			{ label: 'b', description: '' }
		]
	}
];

describe('QuestionRequest', () => {
	it('enables Answer only when every question is answered and sends the labels', async () => {
		const onAnswer = vi.fn();
		render(QuestionRequest, { id: 'q1', questions, status: 'pending', epoch: 3, onAnswer });
		const submit = page.getByRole('button', { name: 'Answer' });
		await expect.element(submit).toBeDisabled();
		await page.getByRole('radio', { name: 'postgres' }).click();
		await expect.element(submit).toBeDisabled();
		await page.getByRole('checkbox', { name: 'b' }).click();
		await page.getByRole('checkbox', { name: 'a' }).click();
		await expect.element(submit).toBeEnabled();
		await submit.click();
		expect(onAnswer).toHaveBeenCalledExactlyOnceWith(
			'q1',
			[
				{ question: 'Which DB?', selected: ['postgres'], other: null },
				// the agent's option order, not the click order
				{ question: 'Extras?', selected: ['a', 'b'], other: null }
			],
			3
		);
	});

	it('accepts a free-text answer instead of an option', async () => {
		const onAnswer = vi.fn();
		render(QuestionRequest, {
			id: 'q1',
			questions: [questions[0]],
			status: 'pending',
			epoch: 1,
			onAnswer
		});
		await page.getByRole('textbox', { name: 'Other' }).fill('  mysql ');
		await page.getByRole('button', { name: 'Answer' }).click();
		expect(onAnswer).toHaveBeenCalledWith(
			'q1',
			[{ question: 'Which DB?', selected: [], other: 'mysql' }],
			1
		);
	});

	it('Skip sends null', async () => {
		const onAnswer = vi.fn();
		render(QuestionRequest, { id: 'q1', questions, status: 'pending', epoch: 1, onAnswer });
		await page.getByRole('button', { name: 'Skip' }).click();
		expect(onAnswer).toHaveBeenCalledWith('q1', null, 1);
	});

	it('shows the outcome without controls once settled', async () => {
		render(QuestionRequest, {
			id: 'q1',
			questions,
			status: 'answered',
			epoch: 1,
			onAnswer: vi.fn()
		});
		await expect.element(page.getByText('Answered')).toBeVisible();
		expect(document.querySelector('button')).toBeNull();
	});
});
