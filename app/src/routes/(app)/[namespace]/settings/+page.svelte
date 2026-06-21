<script lang="ts">
	import { page } from '$app/stores';
	import {
		getMembers,
		getRoles,
		getPrivilegeCatalog,
		setMemberRole,
		updateRolePrivileges,
		createRole,
		deleteRole
	} from '$lib/remote/members.remote';
	import { PRIVILEGE_LABELS } from '$lib/privileges';
	import { confirm } from '$lib/confirm';

	const orgMembership = $derived(
		$page.data.session.orgs.find((o: { slug: string }) => o.slug === $page.params.namespace)
	);
	const privileges = $derived((orgMembership?.privileges ?? []) as string[]);
	const canManageMembers = $derived(privileges.includes('member.manage'));
	const canManageRoles = $derived(privileges.includes('role.manage'));

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
			<code class="mono">{$page.params.namespace}</code>
		</div>
		<div class="info-row">
			<span class="label">Your role</span>
			<code class="mono">{orgMembership?.role ?? '–'}</code>
		</div>
	</div>
</section>

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
	.roles { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 1rem; }
	.role-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.75rem; }
	.role-name { font-weight: 600; display: flex; align-items: center; gap: 0.5rem; }
	.builtin { font-size: 10px; text-transform: uppercase; }
	.priv-grid { display: flex; flex-direction: column; gap: 0.375rem; }
	.priv { display: flex; align-items: center; gap: 0.5rem; font-size: 13px; color: var(--color-text); }
	.role-actions { margin-top: 0.75rem; }
	.btn-sm { font-size: 12px; padding: 0.25rem 0.625rem; }
	.new-role { display: flex; gap: 0.5rem; margin-top: 1rem; }
	.new-role input { font-size: 13px; padding: 0.375rem 0.5rem; background: var(--color-surface-2); color: var(--color-text); border: 1px solid var(--color-border); border-radius: 4px; }
</style>
