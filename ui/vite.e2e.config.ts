import path from 'node:path';
import { mergeConfig } from 'vite';
import { defineConfig } from 'vitest/config';
import base from './vite.config.ts';

// The e2e web server: the app as is, except that the Kubernetes-backed project store is replaced by
// an in-memory one (e2e/stubs/kube.ts), so no production code needs a test switch.
export default mergeConfig(
	base,
	defineConfig({
		resolve: {
			alias: [
				{
					find: /^(#lib\/server|\.)\/kube\.ts$/,
					replacement: path.resolve(import.meta.dirname, 'e2e/stubs/kube.ts')
				}
			]
		}
	})
);
