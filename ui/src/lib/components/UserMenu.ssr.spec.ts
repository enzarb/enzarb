import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';
import UserMenu from './UserMenu.svelte';

describe('UserMenu without JavaScript', () => {
	it('server-renders a plain POST sign-out form in <noscript>', () => {
		const { body } = render(UserMenu, { props: { login: 'ruckc', name: null, avatarUrl: null } });
		const noscript = body.match(/<noscript>([\s\S]*?)<\/noscript>/)?.[1] ?? '';
		expect(noscript).toContain('method="POST"');
		expect(noscript).toContain('action="/logout"');
		expect(noscript).toContain('Sign out');
	});
});
