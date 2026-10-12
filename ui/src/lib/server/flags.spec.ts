import { describe, expect, it } from 'vitest';
import { chatEnabled } from './flags.ts';

describe('chatEnabled', () => {
	it('is on only for the exact value true', () => {
		expect(chatEnabled({ ENZARB_UI_CHAT: 'true' })).toBe(true);
		for (const v of [undefined, '', 'false', '1', 'TRUE', 'True', ' true', 'yes', 'on'])
			expect(chatEnabled({ ENZARB_UI_CHAT: v })).toBe(false);
		expect(chatEnabled({})).toBe(false);
	});
});
