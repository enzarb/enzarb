<script lang="ts">
	import type { Component } from 'svelte';
	import { setProjectContext } from '#lib/project-context.svelte.ts';
	import type { ProjectView } from '#lib/project-types.ts';

	// Hosts a project tab page with the context the layout would provide.
	let { Child, project }: { Child: Component; project: ProjectView } = $props();
	const ctx = $state({ trackingResources: false });
	setProjectContext({
		get project() {
			return project;
		},
		get trackingResources() {
			return ctx.trackingResources;
		},
		set trackingResources(v) {
			ctx.trackingResources = v;
		}
	});
</script>

<Child />
