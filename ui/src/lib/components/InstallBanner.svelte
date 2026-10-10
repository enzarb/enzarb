<script lang="ts">
	/** Shown when the user can reach no repository through an installation of the GitHub App. */
	// The snapshot behind this banner also bounds which repositories may be put in a new Project
	// (24 hours, see project-onboarding.ts); it is not used for any other decision.
	let { installUrl, refreshHref }: { installUrl: string; refreshHref: string } = $props();
</script>

<div class="install-banner" role="status">
	<p>
		Install the enzarb GitHub App on your account or organization to use your repositories, then
		refresh the list.
	</p>
	<p class="install-banner-actions">
		<a class="button primary" href={installUrl} target="_blank" rel="noopener noreferrer">
			Install the GitHub App<span class="visually-hidden"> (opens in a new tab)</span>
		</a>
		<a class="button" href={refreshHref} data-sveltekit-reload>Refresh repositories</a>
	</p>
</div>
