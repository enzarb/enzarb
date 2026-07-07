import adapter from '@sveltejs/adapter-node';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	build: {
		sourcemap: true,
		// xterm.js (+ addons) and shiki are individually large third-party
		// bundles but are already dynamically imported (TilingPane.svelte)
		// so neither ships to a user who never opens a terminal or views a
		// file — the default 500kB warning is about avoidable eager bloat,
		// which doesn't apply here. Raised just above xterm's ~764kB chunk.
		chunkSizeWarningLimit: 900
	},
	plugins: [
		sveltekit({
			experimental: {
				remoteFunctions: true
			},
			// SvelteKit owns CSP so it can nonce its own inline (hydration) scripts.
			// mode 'auto' = nonces for server-rendered inline scripts, hashes for static.
			csp: {
				mode: 'auto',
				directives: {
					'default-src': ['self'],
					'script-src': ['self'],
					// unsafe-inline is required for dynamic CSS custom properties (style:--var=value)
					// used for runtime widths/colors. Static inline styles have been removed.
					// CSS custom property values cannot trigger CSS exfiltration attacks.
					'style-src': ['self', 'unsafe-inline'],
					'img-src': ['self', 'data:'],
					'frame-ancestors': ['none']
				}
			},
			compilerOptions: {
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true,
				experimental: {
					async: true
				}
			},
			adapter: adapter()
		})
	]
});
