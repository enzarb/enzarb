<script lang="ts">
	let { project, fill = false }: { project: string; fill?: boolean } = $props();
</script>

<div data-testid="terminal" data-project={project} data-fill={fill}></div>
