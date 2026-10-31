<script lang="ts">
	import { runLabel } from '#lib/github-links.ts';

	/** A run as `review-pull-requests #1a2b3c4d`, linking to its page. */
	let { workflow, id, href }: { workflow: string; id: string; href: string } = $props();
</script>

<a class="ez-run-link" {href}>{runLabel(workflow, id)}</a>
