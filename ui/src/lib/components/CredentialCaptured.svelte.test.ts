import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import CredentialCaptured from './CredentialCaptured.svelte';

describe('CredentialCaptured', () => {
	it('shows success alone when the token was recorded', async () => {
		render(CredentialCaptured, { name: 'alpha' });
		await expect.element(page.getByRole('status')).toHaveTextContent('Claude token configured');
		await expect.element(page.getByRole('alert')).not.toBeInTheDocument();
		await expect
			.element(page.getByRole('link', { name: 'Back to the Project' }))
			.toHaveAttribute('href', '/projects/alpha');
	});

	it('reports a recording failure in its own error notice, outside the success one', async () => {
		render(CredentialCaptured, { name: 'alpha', saveError: 'could not be recorded' });
		await expect.element(page.getByRole('alert')).toHaveTextContent('could not be recorded');
		await expect.element(page.getByRole('status')).not.toHaveTextContent('could not be recorded');
	});
});
