<script lang="ts">
	import type { PageData, ActionData } from './$types';
	import { enhance } from '$app/forms';
	let { data, form }: { data: PageData; form: ActionData } = $props();
	let showNewOrg = $state(false);
	let inviteOrgId: string | null = $state(null);
</script>

<h2>Admin</h2>

<section class="section">
	<div class="section-header">
		<h3>Organizations</h3>
		<button class="btn btn-primary" onclick={() => showNewOrg = !showNewOrg}>New org</button>
	</div>

	{#if showNewOrg}
		<form method="POST" action="?/createOrg" use:enhance class="card form-card">
			<div class="fields">
				<div class="field"><label>Slug</label><input name="slug" required pattern="[a-z0-9-]+" /></div>
				<div class="field"><label>Display name</label><input name="displayName" required /></div>
				<div class="field">
					<label>Tier</label>
					<select name="tier">
						<option value="free">Free</option>
						<option value="pro">Pro</option>
					</select>
				</div>
			</div>
			<div class="actions">
				<button type="button" class="btn" onclick={() => showNewOrg = false}>Cancel</button>
				<button type="submit" class="btn btn-primary">Create</button>
			</div>
		</form>
	{/if}

	<table>
		<thead><tr><th>Slug</th><th>Display name</th><th>Tier</th><th>Members</th><th>Actions</th></tr></thead>
		<tbody>
			{#each data.orgs as org}
				<tr>
					<td><code class="mono">{org.slug}</code></td>
					<td>{org.display_name}</td>
					<td>
						<form method="POST" action="?/setTier" use:enhance style="display:inline">
							<input type="hidden" name="orgId" value={org.id} />
							<select name="tier" onchange={(e) => (e.target as HTMLSelectElement).form?.submit()} style="width:auto">
								<option value="free" selected={org.tier === 'free'}>free</option>
								<option value="pro" selected={org.tier === 'pro'}>pro</option>
							</select>
						</form>
					</td>
					<td>{org.member_count}</td>
					<td>
						<button class="btn" onclick={() => inviteOrgId = org.id}>Invite</button>
					</td>
				</tr>
				{#if inviteOrgId === org.id}
					<tr>
						<td colspan="5">
							<form method="POST" action="?/invite" use:enhance class="inline-form">
								<input type="hidden" name="orgId" value={org.id} />
								<input name="email" type="email" placeholder="user@example.com" required />
								<select name="role" style="width:auto">
									<option value="member">Member</option>
									<option value="admin">Admin</option>
								</select>
								<button type="submit" class="btn btn-primary">Invite</button>
								<button type="button" class="btn" onclick={() => inviteOrgId = null}>Cancel</button>
							</form>
						</td>
					</tr>
				{/if}
			{:else}
				<tr><td colspan="5" class="muted">No organizations</td></tr>
			{/each}
		</tbody>
	</table>
</section>

<section class="section">
	<h3>Users</h3>
	<table>
		<thead><tr><th>Email</th><th>Admin</th><th>Joined</th></tr></thead>
		<tbody>
			{#each data.users as user}
				<tr>
					<td>{user.email}</td>
					<td>{user.is_admin ? '✓' : ''}</td>
					<td class="muted">{new Date(user.created_at).toLocaleDateString()}</td>
				</tr>
			{:else}
				<tr><td colspan="3" class="muted">No users</td></tr>
			{/each}
		</tbody>
	</table>
</section>

<style>
	.section { margin-bottom: 2rem; }
	.section-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.75rem; }
	.section h3 { font-size: 14px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.06em; }
	.form-card { margin-bottom: 1rem; }
	.fields { display: grid; grid-template-columns: repeat(3, 1fr); gap: 1rem; margin-bottom: 1rem; }
	.field label { display: block; font-weight: 500; margin-bottom: 0.25rem; font-size: 13px; }
	.actions { display: flex; gap: 0.5rem; justify-content: flex-end; }
	.inline-form { display: flex; gap: 0.5rem; align-items: center; padding: 0.5rem 0; }
	.inline-form input { max-width: 220px; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
</style>
