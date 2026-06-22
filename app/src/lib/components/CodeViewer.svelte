<script lang="ts">
	import { getSingletonHighlighter } from 'shiki';

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

	let html = $state('');

	$effect(() => {
		const lang = language ?? detectLang(filename);
		const src = content;
		let cancelled = false;
		(async () => {
			try {
				const hl = await getSingletonHighlighter({ themes: [THEME], langs: [lang] });
				const out = hl.codeToHtml(src, { lang, theme: THEME });
				if (!cancelled) html = out;
			} catch {
				try {
					const hl = await getSingletonHighlighter({ themes: [THEME], langs: ['plaintext'] });
					const out = hl.codeToHtml(src, { lang: 'plaintext', theme: THEME });
					if (!cancelled) html = out;
				} catch {
					if (!cancelled) html = `<pre>${src.replace(/</g, '&lt;')}</pre>`;
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
		{@html html}
	</div>
{/if}

<style>
	.cv-loading {
		color: var(--color-text-muted);
		font-size: 13px;
		padding: 1rem;
	}
	.cv-wrap :global(pre) {
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
	.cv-wrap :global(code) {
		font-family: inherit;
		font-size: inherit;
		background: transparent !important;
	}
</style>
