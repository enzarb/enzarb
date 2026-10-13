import { describe, expect, it, vi } from 'vitest';
import { clearClaudeToken } from './credential-clear.ts';

function fakes(answers: boolean[]) {
	const session = { credentialClear: vi.fn(), close: vi.fn() };
	return {
		session,
		deps: {
			connect: vi.fn(async () => session),
			stillConfigured: vi.fn(async () => answers.shift() ?? true),
			wait: async () => {}
		}
	};
}

describe('clearClaudeToken', () => {
	it('sends credential_clear through the chosen sandbox and waits until the row is gone', async () => {
		const { session, deps } = fakes([true, false]);
		expect(await clearClaudeToken('web', deps)).toBe(true);
		expect(deps.connect).toHaveBeenCalledWith('web');
		expect(session.credentialClear).toHaveBeenCalledOnce();
		expect(deps.stillConfigured).toHaveBeenCalledTimes(2);
		expect(session.close).toHaveBeenCalled();
	});

	it('gives up after the attempts and still closes the connection', async () => {
		const { session, deps } = fakes([]);
		expect(await clearClaudeToken('web', deps, 3)).toBe(false);
		expect(deps.stillConfigured).toHaveBeenCalledTimes(3);
		expect(session.close).toHaveBeenCalled();
	});
});
