<script lang="ts">
	import { getSingletonHighlighter, type ThemedToken } from 'shiki';
	import type { GiteaBlameSection } from '$lib/gitea';

	let { sections, filename }: { sections: GiteaBlameSection[]; filename: string } = $props();

	const EXT_MAP: Record<string, string> = {
		js: 'javascript', mjs: 'javascript', cjs: 'javascript',
		ts: 'typescript', mts: 'typescript', cts: 'typescript',
		svelte: 'svelte', json: 'json', jsonc: 'jsonc', toml: 'toml',
		rs: 'rust', c: 'c', h: 'c', cpp: 'cpp', cc: 'cpp', cxx: 'cpp', hpp: 'cpp',
		md: 'markdown', txt: 'plaintext', sh: 'bash', bash: 'bash', zsh: 'bash',
		py: 'python', yaml: 'yaml', yml: 'yaml', css: 'css', scss: 'scss',
		html: 'html', go: 'go', java: 'java', sql: 'sql', xml: 'xml',
	};
	const THEME = 'one-dark-pro';

	function detectLang(name: string): string {
		const base = name.split('/').pop() ?? name;
		const lower = base.toLowerCase();
		if (lower === 'dockerfile') return 'dockerfile';
		if (lower === 'makefile') return 'makefile';
		const ext = base.split('.').pop()?.toLowerCase() ?? '';
		return EXT_MAP[ext] ?? 'plaintext';
	}

	type BlameLine = {
		lineNum: number;
		tokens: ThemedToken[];
		sha: string;
		author: string;
		date: string;
		firstInGroup: boolean;
	};

	let flatLines = $state<BlameLine[]>([]);

	$effect(() => {
		const secs = sections;
		const file = filename;
		let cancelled = false;
		(async () => {
			const lang = detectLang(file);
			const fullContent = secs.flatMap(s => s.lines).join('\n');
			let tokenLines: ThemedToken[][] = [];
			try {
				const hl = await getSingletonHighlighter({ themes: [THEME], langs: [lang as import('shiki').BundledLanguage] });
				const result = hl.codeToTokens(fullContent, { lang: lang as import('shiki').BundledLanguage, theme: THEME });
				tokenLines = result.tokens;
			} catch {
				// fallback: each line is a single plain token
				tokenLines = fullContent.split('\n').map(l => [{ content: l, color: 'var(--color-text)' } as ThemedToken]);
			}
			if (cancelled) return;
			const result: BlameLine[] = [];
			let lineIdx = 0;
			for (const sec of secs) {
				const sha = sec.commit.sha.slice(0, 7);
				const author = sec.commit.commit.author.name;
				const date = new Date(sec.commit.commit.author.date).toLocaleDateString();
				for (let i = 0; i < sec.lines.length; i++) {
					result.push({
						lineNum: lineIdx + 1,
						tokens: tokenLines[lineIdx] ?? [],
						sha,
						author,
						date,
						firstInGroup: i === 0
					});
					lineIdx++;
				}
			}
			if (!cancelled) flatLines = result;
		})();
		return () => { cancelled = true; };
	});
</script>

<div class="blame-wrap">
	<table class="blame-table">
		<tbody>
			{#each flatLines as line (line.lineNum)}
				<tr>
					<td class="blame-meta">
						{#if line.firstInGroup}
							<span class="blame-sha">{line.sha}</span>
							<span class="blame-author">{line.author}</span>
							<span class="blame-date">{line.date}</span>
						{/if}
					</td>
					<td class="blame-lineno">{line.lineNum}</td>
					<td class="blame-code">
						{#each line.tokens as tok}
							<span style="color:{tok.color ?? 'inherit'}">{tok.content}</span>
						{/each}
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>

<style>
	.blame-wrap {
		overflow-x: auto;
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
	}
	.blame-table {
		width: 100%;
		border-collapse: collapse;
		font-family: var(--font-mono);
		font-size: 12px;
		background: var(--color-surface);
	}
	.blame-table tr:hover {
		background: var(--color-surface-2);
	}
	.blame-table td {
		padding: 0 0.25rem;
		vertical-align: top;
		border-top: 1px solid transparent;
		line-height: 1.6;
	}
	.blame-meta {
		min-width: 220px;
		max-width: 220px;
		color: var(--color-text-muted);
		white-space: nowrap;
		padding: 0 0.5rem 0 0.75rem;
		border-right: 1px solid var(--color-border);
		overflow: hidden;
	}
	.blame-sha {
		color: var(--color-accent);
		margin-right: 0.4rem;
	}
	.blame-author {
		margin-right: 0.4rem;
		font-size: 11px;
	}
	.blame-date {
		font-size: 11px;
		color: var(--color-text-muted);
	}
	.blame-lineno {
		text-align: right;
		color: var(--color-text-muted);
		user-select: none;
		padding: 0 0.75rem;
		min-width: 4ch;
		border-right: 1px solid var(--color-border);
	}
	.blame-code {
		white-space: pre;
		padding: 0 0.75rem;
	}
</style>
