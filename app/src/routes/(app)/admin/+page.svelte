<script lang="ts">
	import { listOrgs, listDeletedOrgs, listUsers, createOrgAdmin, setOrgTier, deleteOrg, recoverOrg, inviteMember, getAdminSettings, updateAdminSettings } from '$lib/remote/admin.remote';
	let showNewOrg = $state(false);
	let inviteOrgId: string | null = $state(null);
</script>

<h2>Admin</h2>

<section class="section">
	<h3>Platform settings</h3>
	{#await getAdminSettings() then settings}
		<div class="card form-card">
			<form {...updateAdminSettings}>
				<h4>Free tier</h4>
				<div class="fields">
					<div class="field">
						<label for="set-pvc">Max workspace storage (GiB)</label>
						<input id="set-pvc" {...updateAdminSettings.fields.freeMaxPvcGi.as('text', String(settings.freeMaxPvcGi))} min="1" required />
						{#each updateAdminSettings.fields.freeMaxPvcGi.issues() as issue}<p class="field-error">{issue.message}</p>{/each}
					</div>
					<div class="field">
						<label for="set-retention">Deletion retention (days)</label>
						<input id="set-retention" {...updateAdminSettings.fields.retentionDays.as('text', String(settings.retentionDays))} min="1" required />
						{#each updateAdminSettings.fields.retentionDays.issues() as issue}<p class="field-error">{issue.message}</p>{/each}
					</div>
					<div class="field">
						<label for="set-free-cpu">Free CPU seconds / month</label>
						<input id="set-free-cpu" {...updateAdminSettings.fields.freeCPUSeconds.as('text', String(settings.pricing.freeCPUSeconds))} min="0" required />
					</div>
					<div class="field">
						<label for="set-free-mem">Free memory GiB-seconds / month</label>
						<input id="set-free-mem" {...updateAdminSettings.fields.freeMemGiBSeconds.as('text', String(settings.pricing.freeMemGiBSeconds))} min="0" required />
					</div>
				</div>

				<h4>Billing rates</h4>
				<div class="fields">
					<div class="field">
						<label for="set-cpu">CPU $ / second</label>
						<input id="set-cpu" {...updateAdminSettings.fields.cpuSecondsPerUnit.as('text', String(settings.pricing.cpuSecondsPerUnit))} step="any" min="0" required />
					</div>
					<div class="field">
						<label for="set-mem">Memory $ / GiB-second</label>
						<input id="set-mem" {...updateAdminSettings.fields.memGiBSecondsPerUnit.as('text', String(settings.pricing.memGiBSecondsPerUnit))} step="any" min="0" required />
					</div>
					<div class="field">
						<label for="set-storage">Storage $ / GiB-second</label>
						<input id="set-storage" {...updateAdminSettings.fields.storageGiBSecondsPerUnit.as('text', String(settings.pricing.storageGiBSecondsPerUnit))} step="any" min="0" required />
					</div>
					<div class="field">
						<label for="set-ingress">Network ingress $ / GiB</label>
						<input id="set-ingress" {...updateAdminSettings.fields.netIngressPerGib.as('text', String(settings.pricing.netIngressPerGib))} step="any" min="0" required />
					</div>
					<div class="field">
						<label for="set-egress">Network egress $ / GiB</label>
						<input id="set-egress" {...updateAdminSettings.fields.netEgressPerGib.as('text', String(settings.pricing.netEgressPerGib))} step="any" min="0" required />
					</div>
				</div>
				<div class="actions">
					<button type="submit" class="btn btn-primary">Save settings</button>
				</div>
			</form>
		</div>
	{/await}
</section>

<section class="section">
	<div class="section-header">
		<h3>Organizations</h3>
		<button class="btn btn-primary" onclick={() => (showNewOrg = !showNewOrg)}>New org</button>
	</div>

	{#if showNewOrg}
		<div class="card form-card">
			<form {...createOrgAdmin}>
				<div class="fields">
					<div class="field">
						<label for="admin-slug">Slug</label>
						<input id="admin-slug" {...createOrgAdmin.fields.slug.as('text')} required pattern="[a-z0-9-]+" />
						{#each createOrgAdmin.fields.slug.issues() as issue}<p class="field-error">{issue.message}</p>{/each}
					</div>
					<div class="field">
						<label for="admin-name">Display name</label>
						<input id="admin-name" {...createOrgAdmin.fields.displayName.as('text')} required />
						{#each createOrgAdmin.fields.displayName.issues() as issue}<p class="field-error">{issue.message}</p>{/each}
					</div>
					<div class="field">
						<label for="admin-tier">Tier</label>
						<select id="admin-tier" {...createOrgAdmin.fields.tier.as('select')}>
							<option value="free">Free</option>
							<option value="pro">Pro</option>
						</select>
					</div>
				</div>
				<div class="actions">
					<button type="button" class="btn" onclick={() => (showNewOrg = false)}>Cancel</button>
					<button type="submit" class="btn btn-primary">Create</button>
				</div>
			</form>
		</div>
	{/if}

	<table>
		<thead><tr><th>Slug</th><th>Display name</th><th>Tier</th><th>Members</th><th>Actions</th></tr></thead>
		<tbody>
			{#each await listOrgs() as org}
				<tr>
					<td><code class="mono">{org.slug}</code></td>
					<td>{org.display_name}</td>
					<td>
						<select
							value={org.tier}
							onchange={async (e) => setOrgTier({ orgId: org.id, tier: (e.target as HTMLSelectElement).value as 'free' | 'pro' })}
							style="width:auto"
						>
							<option value="free">free</option>
							<option value="pro">pro</option>
						</select>
					</td>
					<td>{org.member_count}</td>
					<td>
						<button class="btn" onclick={() => (inviteOrgId = org.id)}>Invite</button>
						<button
							class="btn btn-danger"
							onclick={async () => {
								if (confirm(`Delete org "${org.slug}"? It can be recovered within the retention window.`)) {
									await deleteOrg({ orgId: org.id });
									await listOrgs().refresh();
									await listDeletedOrgs().refresh();
								}
							}}>Delete</button>
					</td>
				</tr>
				{#if inviteOrgId === org.id}
					{@const invite = inviteMember.for(org.id)}
					<tr>
						<td colspan="5">
							<form {...invite} class="inline-form">
								<input {...invite.fields.orgId.as('hidden', org.id)} />

								<input {...invite.fields.email.as('email')} placeholder="user@example.com" required />
								<select {...invite.fields.role.as('select')} style="width:auto">
									<option value="member">Member</option>
									<option value="admin">Admin</option>
								</select>
								<button type="submit" class="btn btn-primary">Invite</button>
								<button type="button" class="btn" onclick={() => (inviteOrgId = null)}>Cancel</button>
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

{#await listDeletedOrgs() then deletedOrgs}
	{#if deletedOrgs.length}
		<section class="section">
			<h3>Deleted organizations</h3>
			<table>
				<thead><tr><th>Slug</th><th>Display name</th><th>Deleted</th><th>Actions</th></tr></thead>
				<tbody>
					{#each deletedOrgs as org}
						<tr>
							<td><code class="mono">{org.slug}</code></td>
							<td>{org.display_name}</td>
							<td class="muted">{new Date(org.deleted_at).toLocaleString()}</td>
							<td>
								<button
									class="btn"
									onclick={async () => {
										await recoverOrg({ orgId: org.id });
										await listOrgs().refresh();
										await listDeletedOrgs().refresh();
									}}>Recover</button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{/if}
{/await}

<section class="section">
	<h3>Users</h3>
	<table>
		<thead><tr><th>Email</th><th>Admin</th><th>Joined</th></tr></thead>
		<tbody>
			{#each await listUsers() as user}
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
	.form-card h4 { font-size: 13px; font-weight: 600; margin: 0.5rem 0; }
	.btn-danger { color: var(--color-danger); border-color: var(--color-danger); }
	.fields { display: grid; grid-template-columns: repeat(3, 1fr); gap: 1rem; margin-bottom: 1rem; }
	.field label { display: block; font-weight: 500; margin-bottom: 0.25rem; font-size: 13px; }
	.field-error { color: var(--color-danger); font-size: 12px; margin: 0.25rem 0 0; }
	.actions { display: flex; gap: 0.5rem; justify-content: flex-end; }
	.inline-form { display: flex; gap: 0.5rem; align-items: center; padding: 0.5rem 0; }
	.inline-form input[type=email] { max-width: 220px; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
</style>
