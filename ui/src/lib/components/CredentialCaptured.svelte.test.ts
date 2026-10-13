import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import CredentialCaptured from './CredentialCaptured.svelte';

describe('CredentialCaptured', () => {
	it('shows success', async () => {
		render(CredentialCaptured);
		await expect.element(page.getByRole('status')).toHaveTextContent('Claude token configured');
		await expect.element(page.getByRole('alert')).not.toBeInTheDocument();
	});
});
