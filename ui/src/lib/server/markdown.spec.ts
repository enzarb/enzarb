import { describe, expect, it } from 'vitest';
import { renderMarkdown } from './markdown.ts';

describe('renderMarkdown', () => {
	it('renders common markdown', () => {
		const html = renderMarkdown('## Plan\n\n- **a**\n- `b`');
		expect(html).toContain('<h2>Plan</h2>');
		expect(html).toContain('<strong>a</strong>');
		expect(html).toContain('<code>b</code>');
	});
	it('strips scripts, handlers, images, iframes and javascript: links', () => {
		const html = renderMarkdown(
			[
				'<script>alert(1)</script>',
				'<img src=x onerror="alert(2)">',
				'<iframe src="https://evil"></iframe>',
				'[click](javascript:alert(3))',
				'<a href="https://ok.example" onclick="x()">ok</a>',
				'![pic](https://x/y.png)'
			].join('\n\n')
		);
		expect(html).not.toMatch(/<script|<img|<iframe|onerror|onclick|javascript:/i);
		expect(html).toContain('href="https://ok.example"');
	});
	it('hardens external links', () => {
		expect(renderMarkdown('[a](https://example.com)')).toContain(
			'rel="noopener noreferrer nofollow"'
		);
	});
});
