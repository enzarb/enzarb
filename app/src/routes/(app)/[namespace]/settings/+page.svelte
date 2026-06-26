<script lang="ts">
	import { page } from '$app/state';
	import {
		getMembers,
		getRoles,
		getPrivilegeCatalog,
		setMemberRole,
		updateRolePrivileges,
		createRole,
		deleteRole
	} from '$lib/remote/members.remote';
	import {
		getUserSecrets,
		setUserSecret,
		deleteUserSecret,
		getGithubOAuthConfig,
		getGithubConnection,
		disconnectGithub
	} from '$lib/remote/settings.remote';
	import { PRIVILEGE_LABELS } from '$lib/privileges';
	import { confirm } from '$lib/confirm';
	import { config } from '$lib/config';

	const orgMembership = $derived(
		page.data.session.orgs.find((o: { slug: string }) => o.slug === page.params.namespace)
	);
	const privileges = $derived((orgMembership?.privileges ?? []) as string[]);
	const canManageMembers = $derived(privileges.includes('member.manage'));
	const canManageRoles = $derived(privileges.includes('role.manage'));
	// Personal orgs are single-user: no team membership or role management.
	const isPersonal = $derived(orgMembership?.personal ?? false);

	let busy = $state('');
	let errorMsg = $state('');

	async function run(key: string, fn: () => Promise<unknown>) {
		busy = key;
		errorMsg = '';
		try {
			await fn();
		} catch (e) {
			errorMsg = e instanceof Error ? e.message : 'Action failed';
		} finally {
			busy = '';
		}
	}

	async function changeRole(userId: string, role: string) {
		await run(`member:${userId}`, async () => {
			await setMemberRole({ userId, role });
			await getMembers().refresh();
		});
	}

	// Local editable copy of each role's privilege set, keyed by role name.
	let draftPrivs = $state<Record<string, Set<string>>>({});

	function toggle(role: string, current: string[], priv: string) {
		const set = draftPrivs[role] ?? new Set(current);
		if (set.has(priv)) set.delete(priv);
		else set.add(priv);
		draftPrivs = { ...draftPrivs, [role]: new Set(set) };
	}

	function isChecked(role: string, current: string[], priv: string) {
		return (draftPrivs[role] ?? new Set(current)).has(priv);
	}

	async function saveRole(name: string, current: string[]) {
		const set = draftPrivs[name] ?? new Set(current);
		await run(`role:${name}`, async () => {
			await updateRolePrivileges({ name, privileges: [...set] });
			await getRoles().refresh();
		});
	}

	// ── User env secrets ─────────────────────────────────────────────────────
	let newSecretKey = $state('');
	let newSecretValue = $state('');
	let secretBusy = $state('');
	let secretError = $state('');

	async function addSecret() {
		const key = newSecretKey.trim();
		const value = newSecretValue;
		if (!key) return;
		secretBusy = 'add';
		secretError = '';
		try {
			await setUserSecret({ key, value });
			newSecretKey = '';
			newSecretValue = '';
			await getUserSecrets().refresh();
		} catch (e) {
			secretError = e instanceof Error ? e.message : 'Failed to save';
		} finally {
			secretBusy = '';
		}
	}

	async function removeSecret(key: string) {
		const ok = await confirm({ title: `Delete secret "${key}"?`, confirmText: 'Delete', danger: true });
		if (!ok) return;
		secretBusy = `del:${key}`;
		secretError = '';
		try {
			await deleteUserSecret({ key });
			await getUserSecrets().refresh();
		} catch (e) {
			secretError = e instanceof Error ? e.message : 'Failed to delete';
		} finally {
			secretBusy = '';
		}
	}

	// ── GitHub OAuth ──────────────────────────────────────────────────────────
	let githubBusy = $state(false);

	const githubConnection = $derived(getGithubConnection());

	async function handleDisconnectGithub() {
		const ok = await confirm({ title: 'Disconnect GitHub?', message: 'GH_TOKEN and related env vars will be removed from all workspaces on next restart.', confirmText: 'Disconnect', danger: true });
		if (!ok) return;
		githubBusy = true;
		try {
			await disconnectGithub();
			await getGithubConnection().refresh();
		} finally {
			githubBusy = false;
		}
	}

	const githubConnected = $derived(page.url.searchParams.get('github') === 'connected');

	let newRoleName = $state('');
	async function addRole() {
		const name = newRoleName.trim();
		if (!name) return;
		await run('role:new', async () => {
			await createRole({ name, privileges: [] });
			newRoleName = '';
			await getRoles().refresh();
		});
	}

	async function removeRole(name: string) {
		const ok = await confirm({
			title: `Delete role "${name}"?`,
			confirmText: 'Delete',
			danger: true
		});
		if (!ok) return;
		await run(`role:del:${name}`, async () => {
			await deleteRole({ name });
			await getRoles().refresh();
		});
	}
</script>

<h2>Settings</h2>

{#if errorMsg}<p class="error-text">{errorMsg}</p>{/if}

<section class="section">
	<h3>Organization</h3>
	<div class="card info-card">
		<div class="info-row">
			<span class="label">Namespace</span>
			<code class="mono">{page.params.namespace}</code>
		</div>
		<div class="info-row">
			<span class="label">Your role</span>
			<code class="mono">{orgMembership?.role ?? '–'}</code>
		</div>
	</div>
</section>

<section class="section">
	<h3>Environment Variables</h3>
	<p class="section-desc">Secrets injected as environment variables into all your workspaces. Set <code>ANTHROPIC_API_KEY</code>, <code>NPM_TOKEN</code>, or any other credential here. Values are write-only after saving.</p>
	{#if secretError}<p class="error-text">{secretError}</p>{/if}
	{#await getUserSecrets() then secrets}
		{#if secrets.length > 0}
			<div class="card secret-table-wrap">
				<table class="secret-table">
					<thead><tr><th>Key</th><th>Value</th><th></th></tr></thead>
					<tbody>
						{#each secrets as s}
							<tr>
								<td><code class="mono">{s.key}</code></td>
								<td class="muted">••••••••</td>
								<td>
									<button class="btn btn-sm btn-danger-outline" disabled={secretBusy === `del:${s.key}`} onclick={() => removeSecret(s.key)}>Remove</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
		<div class="secret-add card">
			<input class="secret-input" type="text" placeholder="KEY" bind:value={newSecretKey} disabled={!!secretBusy} onkeydown={(e) => e.key === 'Enter' && addSecret()} />
			<input class="secret-input secret-val" type="password" placeholder="value" bind:value={newSecretValue} disabled={!!secretBusy} onkeydown={(e) => e.key === 'Enter' && addSecret()} />
			<button class="btn btn-sm" disabled={!newSecretKey.trim() || !!secretBusy} onclick={addSecret}>
				{secretBusy === 'add' ? 'Saving…' : 'Add'}
			</button>
		</div>
	{/await}
</section>

{#await getGithubOAuthConfig() then ghConfig}
	{#if ghConfig.enabled}
		<section class="section">
			<h3>GitHub</h3>
			<p class="section-desc">Connect your GitHub account to automatically inject <code>GH_TOKEN</code> and <code>GITHUB_TOKEN</code> into workspaces — no <code>gh auth login</code> needed. Git identity (<code>user.name</code>, <code>user.email</code>) is also configured automatically.</p>
			{#if githubConnected}
				<p class="success-text">GitHub connected successfully. Restart your workspace to pick up the new credentials.</p>
			{/if}
			{#await githubConnection then connection}
				{#if connection}
					<div class="card github-card">
						<span class="github-connected">Connected as <strong>{connection.login}</strong></span>
						<button class="btn btn-sm btn-danger-outline" disabled={githubBusy} onclick={handleDisconnectGithub}>
							{githubBusy ? 'Disconnecting…' : 'Disconnect'}
						</button>
					</div>
				{:else}
					<a class="btn btn-github" href="/auth/github/connect">
						<svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M12 0C5.37 0 0 5.37 0 12c0 5.3 3.44 9.8 8.2 11.38.6.1.82-.26.82-.58v-2.04c-3.34.73-4.04-1.61-4.04-1.61-.54-1.38-1.33-1.75-1.33-1.75-1.09-.74.08-.73.08-.73 1.2.08 1.83 1.24 1.83 1.24 1.07 1.83 2.8 1.3 3.49 1 .1-.78.42-1.3.76-1.6-2.67-.3-5.47-1.33-5.47-5.93 0-1.31.47-2.38 1.24-3.22-.12-.3-.54-1.52.12-3.18 0 0 1.01-.32 3.3 1.23a11.5 11.5 0 0 1 3-.4c1.02 0 2.04.13 3 .4 2.28-1.55 3.29-1.23 3.29-1.23.66 1.66.24 2.88.12 3.18.77.84 1.24 1.91 1.24 3.22 0 4.61-2.81 5.63-5.48 5.93.43.37.82 1.1.82 2.22v3.29c0 .32.22.7.83.58C20.57 21.8 24 17.3 24 12c0-6.63-5.37-12-12-12z"/></svg>
						Connect GitHub
					</a>
				{/if}
			{/await}
		</section>
	{/if}
{/await}

{#if !isPersonal}
<section class="section">
	<h3>Members</h3>
	{#await Promise.all([getMembers(), getRoles()]) then [members, roles]}
		<div class="card">
			<table>
				<thead><tr><th>Member</th><th>Role</th></tr></thead>
				<tbody>
					{#each members as m}
						<tr>
							<td>{m.email}</td>
							<td>
								{#if canManageMembers}
									<select
										value={m.role}
										disabled={busy === `member:${m.userId}`}
										onchange={(e) => changeRole(m.userId, e.currentTarget.value)}
									>
										{#each roles as r}
											<option value={r.name}>{r.name}</option>
										{/each}
									</select>
								{:else}
									<span class="badge">{m.role}</span>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/await}
</section>

<section class="section">
	<h3>Roles & privileges</h3>
	{#await Promise.all([getRoles(), getPrivilegeCatalog()]) then [roles, catalog]}
		<div class="roles">
			{#each roles as role}
				<div class="card role-card">
					<div class="role-head">
						<span class="role-name">
							{role.name}
							{#if role.builtin}<span class="badge builtin">builtin</span>{/if}
						</span>
						{#if canManageRoles && !role.builtin}
							<button class="btn btn-sm" disabled={!!busy} onclick={() => removeRole(role.name)}>Delete</button>
						{/if}
					</div>
					<div class="priv-grid">
						{#each catalog as priv}
							<label class="priv">
								<input
									type="checkbox"
									checked={isChecked(role.name, role.privileges, priv)}
									disabled={!canManageRoles}
									onchange={() => toggle(role.name, role.privileges, priv)}
								/>
								<span>{PRIVILEGE_LABELS[priv as keyof typeof PRIVILEGE_LABELS] ?? priv}</span>
							</label>
						{/each}
					</div>
					{#if canManageRoles}
						<div class="role-actions">
							<button
								class="btn btn-primary btn-sm"
								disabled={busy === `role:${role.name}`}
								onclick={() => saveRole(role.name, role.privileges)}
							>
								{busy === `role:${role.name}` ? 'Saving…' : 'Save'}
							</button>
						</div>
					{/if}
				</div>
			{/each}
		</div>

		{#if canManageRoles}
			<div class="new-role">
				<input
					type="text"
					placeholder="new-role-name"
					bind:value={newRoleName}
					disabled={busy === 'role:new'}
				/>
				<button class="btn btn-sm" disabled={busy === 'role:new' || !newRoleName.trim()} onclick={addRole}>
					Add role
				</button>
			</div>
		{/if}
	{/await}
</section>
{/if}

<style>
	.section { margin-bottom: 2rem; }
	.section h3 { font-size: 14px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.06em; margin-bottom: 0.75rem; }
	.info-card { max-width: 480px; }
	.info-row { display: flex; justify-content: space-between; align-items: center; padding: 0.5rem 0; }
	.label { font-size: 13px; color: var(--color-text-muted); }
	.mono { font-family: var(--font-mono); font-size: 13px; }
	.error-text { color: var(--color-danger, #c0392b); font-size: 13px; }
	table { width: 100%; border-collapse: collapse; }
	th, td { text-align: left; padding: 0.5rem 0.25rem; font-size: 13px; border-bottom: 1px solid var(--color-border); }
	th { color: var(--color-text-muted); font-weight: 600; }
	select { font-size: 13px; padding: 0.25rem; background: var(--color-surface-2); color: var(--color-text); border: 1px solid var(--color-border); border-radius: 4px; }
	.roles { display: grid; grid-template-columns: repeat(auto-fill, minmax(420px, 1fr)); gap: 1rem; }
	.role-card { display: flex; flex-direction: column; }
	.role-head { display: flex; justify-content: space-between; align-items: center; gap: 0.5rem; margin-bottom: 0.75rem; padding-bottom: 0.625rem; border-bottom: 1px solid var(--color-border); }
	.role-name { font-weight: 600; display: flex; align-items: center; gap: 0.5rem; }
	.builtin { font-size: 10px; text-transform: uppercase; }
	.priv-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: 0.5rem 1rem; }
	.priv { display: flex; align-items: center; gap: 0.5rem; font-size: 13px; color: var(--color-text); cursor: pointer; }
	.priv input { margin: 0; flex-shrink: 0; }
	.role-actions { margin-top: 1rem; display: flex; justify-content: flex-end; }
	.btn-sm { font-size: 12px; padding: 0.25rem 0.625rem; }
	.new-role { display: flex; gap: 0.5rem; margin-top: 1rem; }
	.new-role input { font-size: 13px; padding: 0.375rem 0.5rem; background: var(--color-surface-2); color: var(--color-text); border: 1px solid var(--color-border); border-radius: 4px; }

	.section-desc { font-size: 13px; color: var(--color-text-muted); margin: 0 0 0.75rem; }
	.section-desc code { font-family: var(--font-mono); font-size: 12px; background: var(--color-surface-2); padding: 0.05rem 0.3rem; border-radius: 3px; color: var(--color-text); }
	.secret-table-wrap { margin-bottom: 0.75rem; }
	.secret-table { width: 100%; border-collapse: collapse; }
	.secret-table th, .secret-table td { padding: 0.4rem 0.5rem; font-size: 13px; border-bottom: 1px solid var(--color-border); text-align: left; }
	.secret-table th { font-size: 11px; color: var(--color-text-muted); text-transform: uppercase; }
	.secret-add { display: flex; gap: 0.5rem; align-items: center; padding: 0.5rem; }
	.secret-input { flex: 1; font-size: 13px; font-family: var(--font-mono); padding: 0.375rem 0.5rem; background: var(--color-surface-2); color: var(--color-text); border: 1px solid var(--color-border); border-radius: 4px; min-width: 0; }
	.secret-val { flex: 2; }
	.btn-danger-outline { color: var(--color-danger, #c0392b); border-color: var(--color-danger, #c0392b); background: none; }
	.btn-danger-outline:hover { background: rgba(192, 57, 43, 0.1); }
	.success-text { color: #3fb950; font-size: 13px; margin-bottom: 0.5rem; }
	.github-card { display: flex; align-items: center; justify-content: space-between; gap: 1rem; padding: 0.75rem; }
	.github-connected { font-size: 13px; }
	.btn-github { display: inline-flex; align-items: center; gap: 0.5rem; padding: 0.4rem 0.75rem; border: 1px solid var(--color-border); border-radius: 4px; background: none; color: var(--color-text); font-size: 13px; text-decoration: none; cursor: pointer; }
	.btn-github:hover { background: var(--color-surface-2); }
</style>
