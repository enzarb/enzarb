<script lang="ts">
	import { page } from '$app/stores';
</script>

<h2>Settings</h2>

<section class="section">
	<h3>Organization</h3>
	<div class="card info-card">
		<div class="info-row">
			<span class="label">Namespace</span>
			<code class="mono">{$page.params.namespace}</code>
		</div>
	</div>
</section>

<style>
	.section { margin-bottom: 2rem; }
	.section h3 { font-size: 14px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.06em; margin-bottom: 0.75rem; }
	.info-card { max-width: 480px; }
	.info-row { display: flex; justify-content: space-between; align-items: center; padding: 0.5rem 0; }
	.label { font-size: 13px; color: var(--color-text-muted); }
	.mono { font-family: var(--font-mono); font-size: 13px; }
</style>
