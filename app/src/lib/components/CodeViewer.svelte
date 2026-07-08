<script lang="ts">
	import { getSingletonHighlighter } from 'shiki';
	import type { ThemedToken } from 'shiki';
	// The default oniguruma engine loads a WASM binary; that fetch is brittle
	// under adapter-node depending on how the built asset gets served, and
	// failing silently there meant every language fell back to unstyled
	// plain text. The pure-JS regex engine avoids WASM entirely.
	import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';

	let {
		content,
		filename,
		language,
		loading = false
	}: {
		content: string;
		filename: string;
		language?: string;
		loading?: boolean;
	} = $props();

	const EXT_MAP: Record<string, string> = {
		js: 'javascript', mjs: 'javascript', cjs: 'javascript',
		ts: 'typescript', mts: 'typescript', cts: 'typescript',
		svelte: 'svelte',
		json: 'json', jsonc: 'jsonc',
		toml: 'toml',
		rs: 'rust',
		c: 'c', h: 'c',
		cpp: 'cpp', cc: 'cpp', cxx: 'cpp', hpp: 'cpp',
		md: 'markdown', mdx: 'mdx',
		txt: 'plaintext',
		sh: 'bash', bash: 'bash', zsh: 'bash',
		py: 'python',
		yaml: 'yaml', yml: 'yaml',
		css: 'css', scss: 'scss',
		html: 'html',
		go: 'go',
		java: 'java',
		kt: 'kotlin',
		rb: 'plaintext',
		sql: 'sql',
		xml: 'xml',
		dockerfile: 'dockerfile',
	};

	const THEME = 'one-dark-pro';

	function detectLang(name: string): string {
		const base = name.split('/').pop() ?? name;
		// handle extensionless files like Dockerfile, Makefile
		const lower = base.toLowerCase();
		if (lower === 'dockerfile') return 'dockerfile';
		if (lower === 'makefile') return 'makefile';
		const ext = base.split('.').pop()?.toLowerCase() ?? '';
		return EXT_MAP[ext] ?? 'plaintext';
	}

	let lines = $state<ThemedToken[][]>([]);
	let bg = $state('');
	let fg = $state('');

	$effect(() => {
		const lang = language ?? detectLang(filename);
		const src = content;
		let cancelled = false;
		(async () => {
			try {
				const hl = await getSingletonHighlighter({
					themes: [THEME],
					langs: [lang],
					engine: createJavaScriptRegexEngine()
				});
				const result = hl.codeToTokens(src, { lang: lang as any, theme: THEME });
				if (!cancelled) {
					lines = result.tokens;
					bg = result.bg ?? '';
					fg = result.fg ?? '';
				}
			} catch {
				try {
					const hl = await getSingletonHighlighter({
						themes: [THEME],
						langs: ['plaintext'],
						engine: createJavaScriptRegexEngine()
					});
					const result = hl.codeToTokens(src, { lang: 'plaintext', theme: THEME });
					if (!cancelled) {
						lines = result.tokens;
						bg = result.bg ?? '';
						fg = result.fg ?? '';
					}
				} catch {
					if (!cancelled) {
						// Plain fallback: single line of unstyled tokens
						lines = [[{ content: src, offset: 0, color: undefined, bgColor: undefined, fontStyle: 0, htmlStyle: undefined }]];
					}
				}
			}
		})();
		return () => { cancelled = true; };
	});
</script>

{#if loading}
	<div class="cv-loading">Loading…</div>
{:else}
	<div class="cv-wrap">
		<pre style:background={bg || undefined} style:color={fg || undefined}><code>{#each lines as line, i}<span class="line">{#each line as token}<span style:color={token.color || undefined}>{token.content}</span>{/each}</span>{#if i < lines.length - 1}{'\n'}{/if}{/each}</code></pre>
	</div>
{/if}

<style>
	.cv-loading {
		color: var(--color-text-muted);
		font-size: 13px;
		padding: 1rem;
	}
	.cv-wrap pre {
		margin: 0;
		padding: 1rem;
		border-radius: var(--radius);
		border: 1px solid var(--color-border);
		font-family: var(--font-mono);
		font-size: 13px;
		line-height: 1.6;
		overflow-x: auto;
		background: var(--color-surface) !important;
	}
	.cv-wrap code {
		font-family: inherit;
		font-size: inherit;
		background: transparent !important;
	}
	.line {
		display: block;
	}
</style>
