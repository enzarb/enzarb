import { marked } from 'marked';
import sanitizeHtml from 'sanitize-html';

const OPTIONS: sanitizeHtml.IOptions = {
	allowedTags: [
		'p',
		'br',
		'hr',
		'strong',
		'em',
		'del',
		'code',
		'pre',
		'blockquote',
		'ul',
		'ol',
		'li',
		'h1',
		'h2',
		'h3',
		'h4',
		'h5',
		'h6',
		'a',
		'table',
		'thead',
		'tbody',
		'tr',
		'th',
		'td'
	],
	allowedAttributes: { a: ['href', 'title', 'rel', 'target'], code: ['class'] },
	allowedClasses: { code: [/^language-[\w-]+$/] },
	allowedSchemes: ['http', 'https', 'mailto'],
	allowProtocolRelative: false,
	transformTags: {
		a: sanitizeHtml.simpleTransform('a', { rel: 'noopener noreferrer nofollow', target: '_blank' })
	}
};

/** Render untrusted (agent-written) markdown to sanitized HTML. */
export function renderMarkdown(markdown: string): string {
	const raw = marked.parse(markdown, { async: false, gfm: true, breaks: false });
	return sanitizeHtml(raw, OPTIONS);
}
