<script lang="ts">
	import { getIssues, createIssueCmd } from '$lib/remote/issues.remote';
	import { page } from '$app/stores';

	let stateFilter = $state<'open' | 'closed'>('open');
	let showNew = $state(false);
	let creating = $state(false);
	let createError = $state('');

	async function handleCreate(e: SubmitEvent) {
		e.preventDefault();
		const fd = new FormData(e.target as HTMLFormElement);
		creating = true;
		createError = '';
		try {
			await createIssueCmd({ title: fd.get('title') as string, body: fd.get('body') as string ?? '' });
			showNew = false;
			(e.target as HTMLFormElement).reset();
		} catch (err) {
			createError = err instanceof Error ? err.message : 'Failed to create issue';
		} finally {
			creating = false;
		}
	}

	function fmtDate(s: string) {
		return new Date(s).toLocaleDateString();
	}
</script>

<div class="issues-page">
	<div class="page-header">
		<div class="state-toggle">
			<button class="toggle-btn {stateFilter === 'open' ? 'active' : ''}" onclick={() => (stateFilter = 'open')}>Open</button>
			<button class="toggle-btn {stateFilter === 'closed' ? 'active' : ''}" onclick={() => (stateFilter = 'closed')}>Closed</button>
		</div>
		<button class="btn btn-primary" onclick={() => (showNew = !showNew)}>New issue</button>
	</div>

	{#if showNew}
		<div class="card new-issue-form">
			<h4>New issue</h4>
			<form onsubmit={handleCreate}>
				<div class="field">
					<label for="issue-title">Title</label>
					<input id="issue-title" name="title" type="text" class="input" required placeholder="Issue title" />
				</div>
				<div class="field">
					<label for="issue-body">Description</label>
					<textarea id="issue-body" name="body" class="input" rows="5" placeholder="Describe the issue…"></textarea>
				</div>
				{#if createError}<p class="err">{createError}</p>{/if}
				<div class="form-actions">
					<button type="button" class="btn" onclick={() => (showNew = false)}>Cancel</button>
					<button type="submit" class="btn btn-primary" disabled={creating}>{creating ? 'Creating…' : 'Create'}</button>
				</div>
			</form>
		</div>
	{/if}

	{#await getIssues({ state: stateFilter, page: 1 }) then issues}
		{#if issues.length === 0}
			<p class="muted">No {stateFilter} issues.</p>
		{:else}
			<div class="issue-list">
				{#each issues as issue}
					<a
						href="/{$page.params.namespace}/projects/{$page.params.project}/issues/{issue.number}"
						class="issue-row card"
					>
						<div class="issue-main">
							<span class="issue-title">{issue.title}</span>
							<span class="issue-meta muted">#{issue.number} opened {fmtDate(issue.created_at)} by {issue.user?.login ?? '?'}</span>
						</div>
						<div class="issue-right">
							{#if issue.comments > 0}
								<span class="comment-count muted">💬 {issue.comments}</span>
							{/if}
						</div>
					</a>
				{/each}
			</div>
		{/if}
	{:catch err}
		<p class="muted">Could not load issues: {err?.message ?? 'unknown error'}</p>
	{/await}
</div>

<style>
	.issues-page { display: flex; flex-direction: column; gap: 1rem; }
	.page-header { display: flex; justify-content: space-between; align-items: center; }
	.state-toggle { display: flex; border: 1px solid var(--color-border); border-radius: var(--radius); overflow: hidden; }
	.toggle-btn { padding: 0.35rem 0.9rem; font-size: 13px; background: none; border: none; cursor: pointer; color: var(--color-text-muted); }
	.toggle-btn.active { background: var(--color-surface-2); color: var(--color-text); font-weight: 500; }
	.new-issue-form { max-width: 640px; }
	.new-issue-form h4 { margin-bottom: 1rem; }
	.field { margin-bottom: 0.75rem; }
	label { display: block; font-size: 13px; font-weight: 500; margin-bottom: 0.25rem; }
	.input { width: 100%; padding: 0.4rem 0.6rem; border: 1px solid var(--color-border); border-radius: var(--radius); background: var(--color-bg); color: var(--color-text); font-size: 13px; font-family: inherit; box-sizing: border-box; }
	textarea.input { resize: vertical; }
	.form-actions { display: flex; gap: 0.5rem; justify-content: flex-end; }
	.err { color: var(--color-danger, #c0392b); font-size: 12px; }
	.issue-list { display: flex; flex-direction: column; gap: 0.5rem; }
	.issue-row { display: flex; justify-content: space-between; align-items: center; gap: 1rem; padding: 0.75rem 1rem; text-decoration: none; color: inherit; }
	.issue-row:hover { background: var(--color-surface-2); text-decoration: none; }
	.issue-main { display: flex; flex-direction: column; gap: 0.2rem; min-width: 0; }
	.issue-title { font-size: 14px; font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.issue-meta { font-size: 12px; }
	.issue-right { flex-shrink: 0; }
	.comment-count { font-size: 12px; }
	.muted { color: var(--color-text-muted); }
</style>
