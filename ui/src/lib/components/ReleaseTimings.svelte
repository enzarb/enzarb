<script lang="ts">
	import type { ReleaseTiming } from '#lib/about-types.ts';
	import { formatDuration } from '#lib/duration.ts';
	import { displayVersion } from '#lib/versions.ts';

	/** Newest first; any stage can be missing. */
	let { timings }: { timings: ReleaseTiming[] } = $props();

	const NONE = '—';
	const when = (t: string | null) => (t ? t.slice(0, 16).replace('T', ' ') + 'Z' : NONE);
</script>

{#if timings.length}
	<section>
		<h2>Recent releases</h2>
		<p class="ez-muted">
			Time from the release being published (the merge) to the chart installed on oldie, then to the
			first sandbox picking up a pod-shape change.
		</p>
		<div class="ez-table-wrap">
			<table class="ez-table">
				<thead>
					<tr>
						<th scope="col">Chart</th>
						<th scope="col">Merged</th>
						<th scope="col">Deployed</th>
						<th scope="col">Sandbox applied</th>
					</tr>
				</thead>
				<tbody>
					{#each timings as t (t.version)}
						<tr>
							<td><a href={t.url}>{displayVersion(t.version)}</a></td>
							<td>{when(t.publishedAt)}</td>
							<td>
								{when(t.deployedAt)}
								{#if t.skew}
									<span class="ez-muted">(n/a)</span>
								{:else if t.mergeToDeployedSeconds !== null}
									<span class="ez-muted"
										>({formatDuration(t.mergeToDeployedSeconds)} after merge)</span
									>
								{/if}
							</td>
							<td>
								{when(t.sandboxAppliedAt)}
								{#if t.deployToSandboxSeconds !== null}
									<span class="ez-muted"
										>({formatDuration(t.deployToSandboxSeconds)} after deploy)</span
									>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</section>
{/if}
