<script lang="ts">
	import { listOrgs, listDeletedOrgs, listUsers, listAllProjects, adminDeleteProject, adminForceDeleteProject, createOrgAdmin, setOrgTier, deleteOrg, recoverOrg, inviteMember, getAdminSettings, updateAdminSettings, listErrorLogs } from '$lib/remote/admin.remote';
	import { confirm } from '$lib/confirm';
	let showNewOrg = $state(false);
	let inviteOrgId: string | null = $state(null);
	let errorScope = $state<string | undefined>(undefined);
	let expandedError = $state<string | null>(null);
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
					<div class="field">
						<label for="set-free-storage">Free storage GiB-seconds / month</label>
						<input id="set-free-storage" {...updateAdminSettings.fields.freeStorageGiBSeconds.as('text', String(settings.pricing.freeStorageGiBSeconds))} min="0" required />
					</div>
					<div class="field">
						<label for="set-free-zot">Free registry storage GiB-seconds / month</label>
						<input id="set-free-zot" {...updateAdminSettings.fields.freeZotStorageGiBSeconds.as('text', String(settings.pricing.freeZotStorageGiBSeconds))} min="0" required />
					</div>
					<div class="field">
						<label for="set-free-net-in-int">Free internal ingress GiB / month</label>
						<input id="set-free-net-in-int" {...updateAdminSettings.fields.freeNetIngressInternalGib.as('text', String(settings.pricing.freeNetIngressInternalGib))} min="0" required />
					</div>
					<div class="field">
						<label for="set-free-net-out-int">Free internal egress GiB / month</label>
						<input id="set-free-net-out-int" {...updateAdminSettings.fields.freeNetEgressInternalGib.as('text', String(settings.pricing.freeNetEgressInternalGib))} min="0" required />
					</div>
					<div class="field">
						<label for="set-free-net-in-ext">Free external ingress GiB / month</label>
						<input id="set-free-net-in-ext" {...updateAdminSettings.fields.freeNetIngressExternalGib.as('text', String(settings.pricing.freeNetIngressExternalGib))} min="0" required />
					</div>
					<div class="field">
						<label for="set-free-net-out-ext">Free external egress GiB / month</label>
						<input id="set-free-net-out-ext" {...updateAdminSettings.fields.freeNetEgressExternalGib.as('text', String(settings.pricing.freeNetEgressExternalGib))} min="0" required />
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
						<label for="set-zot-storage">Registry storage $ / GiB-second</label>
						<input id="set-zot-storage" {...updateAdminSettings.fields.zotStorageGiBSecondsPerUnit.as('text', String(settings.pricing.zotStorageGiBSecondsPerUnit))} step="any" min="0" required />
					</div>
					<div class="field">
						<label for="set-net-in-int">Internal ingress $ / GiB</label>
						<input id="set-net-in-int" {...updateAdminSettings.fields.netIngressInternalPerGib.as('text', String(settings.pricing.netIngressInternalPerGib))} step="any" min="0" required />
					</div>
					<div class="field">
						<label for="set-net-out-int">Internal egress $ / GiB</label>
						<input id="set-net-out-int" {...updateAdminSettings.fields.netEgressInternalPerGib.as('text', String(settings.pricing.netEgressInternalPerGib))} step="any" min="0" required />
					</div>
					<div class="field">
						<label for="set-net-in-ext">External ingress $ / GiB</label>
						<input id="set-net-in-ext" {...updateAdminSettings.fields.netIngressExternalPerGib.as('text', String(settings.pricing.netIngressExternalPerGib))} step="any" min="0" required />
					</div>
					<div class="field">
						<label for="set-net-out-ext">External egress $ / GiB</label>
						<input id="set-net-out-ext" {...updateAdminSettings.fields.netEgressExternalPerGib.as('text', String(settings.pricing.netEgressExternalPerGib))} step="any" min="0" required />
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
							class="w-auto"
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
								const ok = await confirm({
									title: `Delete org "${org.slug}"?`,
									message: 'It can be recovered within the retention window.',
									requireText: org.slug,
									confirmText: 'Delete',
									danger: true
								});
								if (ok) {
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
								<select {...invite.fields.role.as('select')} class="w-auto">
									<option value="member">Member</option>
									<option value="manager">Manager</option>
									<option value="owner">Owner</option>
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
	<h3>Projects</h3>
	<table>
		<thead><tr><th>Owner</th><th>Org</th><th>Project</th><th>Status</th><th>Created</th><th>Actions</th></tr></thead>
		<tbody>
			{#each await listAllProjects() as p}
				<tr>
					<td>{#if p.userEmail}{p.userEmail}{:else}<span class="muted">team</span>{/if}</td>
					<td><code class="mono">{p.orgSlug}</code></td>
					<td>{p.displayName}<br /><code class="mono muted">{p.slug}</code></td>
					<td>
						{#if p.deleting}
							<span class="badge badge-danger">Pending full deletion</span>
						{:else if p.purgeAfter}
							<span class="badge">Soft-deleted · purges {new Date(p.purgeAfter).toLocaleDateString()}</span>
						{:else}
							{p.phase || '—'}
						{/if}
					</td>
					<td class="muted">{p.createdAt ? new Date(p.createdAt).toLocaleDateString() : '—'}</td>
					<td>
						{#if p.deleting}
							<button
								class="btn btn-danger"
								onclick={async () => {
									const ok = await confirm({
										title: `Force delete "${p.slug}"?`,
										message: 'This clears the cleanup finalizer and removes the project even if teardown is wedged. Out-of-namespace resources may be orphaned. This cannot be undone.',
										requireText: p.slug,
										confirmText: 'Force delete',
										danger: true
									});
									if (ok) await adminForceDeleteProject({ orgId: p.orgId, slug: p.slug });
								}}>Force delete</button>
						{:else}
							<button
								class="btn btn-danger"
								onclick={async () => {
									const ok = await confirm({
										title: `Delete "${p.slug}"?`,
										message: `Deletes the project in org "${p.orgSlug}" immediately (no retention window).`,
										requireText: p.slug,
										confirmText: 'Delete',
										danger: true
									});
									if (ok) await adminDeleteProject({ orgId: p.orgId, slug: p.slug });
								}}>Delete</button>
						{/if}
					</td>
				</tr>
			{:else}
				<tr><td colspan="6" class="muted">No projects</td></tr>
			{/each}
		</tbody>
	</table>
</section>

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

<section class="section">
	<div class="section-header">
		<h3>Error log</h3>
		<div class="scope-filters">
			{#each [undefined, 'security', 'application', 'client'] as s}
				<button
					class="btn scope-btn"
					class:active={errorScope === s}
					onclick={() => { errorScope = s; }}
				>{s ?? 'All'}</button>
			{/each}
		</div>
	</div>
	{#await listErrorLogs()}
		<p class="muted">Loading…</p>
	{:then allLogs}
		{@const logs = errorScope ? allLogs.filter(l => l.scope === errorScope) : allLogs}
		<table>
			<thead><tr><th>Time</th><th>Scope</th><th>Message</th><th>User</th><th>IP</th></tr></thead>
			<tbody>
				{#each logs as log}
					<tr class="error-row" onclick={() => expandedError = expandedError === log.id ? null : log.id}>
						<td class="muted mono">{new Date(log.created_at).toLocaleString()}</td>
						<td><span class="badge scope-{log.scope}">{log.scope}</span></td>
						<td class="error-msg">{log.message}</td>
						<td class="muted mono">{log.user_id ?? '—'}</td>
						<td class="muted mono">{log.ip_address ?? '—'}</td>
					</tr>
					{#if expandedError === log.id}
						<tr class="error-detail">
							<td colspan="5">
								{#if log.stack}<pre class="error-stack">{log.stack}</pre>{/if}
								<pre class="error-context">{JSON.stringify(log.context, null, 2)}</pre>
							</td>
						</tr>
					{/if}
				{:else}
					<tr><td colspan="5" class="muted">No errors recorded</td></tr>
				{/each}
			</tbody>
		</table>
	{/await}
</section>

<style>
	.section { margin-bottom: 2rem; }
	.section-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.75rem; }
	.section h3 { font-size: 14px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.06em; }
	.form-card { margin-bottom: 1rem; }
	.form-card h4 { font-size: 13px; font-weight: 600; margin: 0.5rem 0; }
	.fields { display: grid; grid-template-columns: repeat(3, 1fr); gap: 1rem; margin-bottom: 1rem; }
	.field label { display: block; font-weight: 500; margin-bottom: 0.25rem; font-size: 13px; }
	.field-error { color: var(--color-danger); font-size: 12px; margin: 0.25rem 0 0; }
	.actions { display: flex; gap: 0.5rem; justify-content: flex-end; }
	.inline-form { display: flex; gap: 0.5rem; align-items: center; padding: 0.5rem 0; }
	.inline-form input[type=email] { max-width: 220px; }
	.w-auto { width: auto; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.badge { display: inline-block; padding: 0.1rem 0.45rem; border-radius: 4px; font-size: 12px; border: 1px solid var(--color-border); color: var(--color-text-muted); }
	.badge-danger { color: var(--color-danger); border-color: var(--color-danger); }
	.alert { padding: 0.75rem 1rem; border-radius: 6px; font-size: 13px; margin-bottom: 1.25rem; }
	.alert-warn { background: color-mix(in srgb, var(--color-warning, #d29922) 12%, transparent); border: 1px solid color-mix(in srgb, var(--color-warning, #d29922) 40%, transparent); color: var(--color-text); }
	.scope-filters { display: flex; gap: 0.25rem; }
	.scope-btn { font-size: 12px; padding: 0.2rem 0.6rem; }
	.scope-btn.active { background: var(--color-accent); color: #fff; border-color: var(--color-accent); }
	.error-row { cursor: pointer; }
	.error-row:hover td { background: color-mix(in srgb, var(--color-accent) 6%, transparent); }
	.error-msg { max-width: 400px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
	.error-detail td { background: var(--color-bg-subtle, #f6f8fa); padding: 0.5rem 1rem; }
	.error-stack { font-size: 11px; font-family: var(--font-mono); white-space: pre-wrap; margin: 0 0 0.5rem; color: var(--color-danger); }
	.error-context { font-size: 11px; font-family: var(--font-mono); white-space: pre-wrap; margin: 0; }
	.scope-security { color: var(--color-danger); border-color: var(--color-danger); }
	.scope-client { color: var(--color-warning, #d29922); border-color: var(--color-warning, #d29922); }
</style>
