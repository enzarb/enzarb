<script lang="ts">
	import { subjectLabel, subjectUrl, type RepoContext } from '#lib/github-links.ts';

	/** An issue or pull request as `enzarb#43`, linking to GitHub. Never a bare "GitHub". */
	let {
		repo,
		number,
		url = null,
		context
	}: {
		/** `owner/repo` */
		repo: string;
		number: number | null;
		/** Defaults to the issue URL, which GitHub redirects for pull requests. */
		url?: string | null;
		context?: RepoContext;
	} = $props();

	const label = $derived(subjectLabel(repo, number, context));
	const href = $derived(url ?? (number ? subjectUrl(repo, number) : `https://github.com/${repo}`));
</script>

<a class="ez-subject-link" {href} target="_blank" rel="noopener noreferrer">{label}</a>
