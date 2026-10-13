<script lang="ts">
	import { SvelteSet } from 'svelte/reactivity';
	import type { ProjectView } from '#lib/project-types.ts';
	import { normalizeRepoKey } from '#lib/projects.ts';
	import Button from './form/Button.svelte';
	import Checkbox from './form/Checkbox.svelte';
	import Notice from './ui/Notice.svelte';
	import StepList, { type StepItem } from './ui/StepList.svelte';

	export interface RepoChoice {
		repo: string;
		private: boolean;
	}
	export type SaveOutcome = { ok: true } | { ok: false; message: string };

	/**
	 * Which repositories belong to the Project. Offers the current ones (always, so a repository
	 * that was archived or left the user's list can still be removed) plus the user's snapshot;
	 * repositories in another of the user's Projects are disabled. Applies without a restart.
	 */
	let {
		project,
		choices,
		taken = {},
		problem = null,
		save,
		canEdit = true
	}: {
		project: ProjectView;
		choices: RepoChoice[];
		/** Repository key → name of the user's Project holding it. */
		taken?: Record<string, string>;
		problem?: string | null;
		save: (keys: string[]) => Promise<SaveOutcome>;
		canEdit?: boolean;
	} = $props();

	const current = $derived(
		project.repositories
			.map((r) => normalizeRepoKey(r.connector, r.url))
			.filter((k): k is string => !!k)
	);
	const currentKey = $derived(current.join(','));
	const rows = $derived.by(() => {
		const seen = new Set<string>(); // eslint-disable-line svelte/prefer-svelte-reactivity -- local to this derivation
		const out: { key: string; label: string; holder?: string; missing: boolean }[] = [];
		for (const key of current) {
			seen.add(key);
			out.push({ key, label: key.replace(/^github:/, ''), missing: false });
		}
		for (const c of choices) {
			const key = c.repo.toLowerCase();
			if (seen.has(key)) continue;
			seen.add(key);
			const holder = taken[key] && taken[key] !== project.name ? taken[key] : undefined;
			out.push({ key, label: key.replace(/^github:/, ''), holder, missing: false });
		}
		const offered = new Set(choices.map((c) => c.repo.toLowerCase()));
		for (const r of out) r.missing = current.includes(r.key) && !offered.has(r.key);
		return out;
	});

	const selected = new SvelteSet<string>();
	$effect(() => {
		void currentKey;
		selected.clear();
		for (const k of current) selected.add(k);
	});

	const keys = $derived(rows.map((r) => r.key).filter((k) => selected.has(k)));
	const dirty = $derived(keys.join(',') !== currentKey);

	type Phase = 'idle' | 'saving' | 'done' | 'failed';
	let phase = $state<Phase>('idle');
	let failure = $state('');

	async function submit() {
		phase = 'saving';
		failure = '';
		try {
			const r = await save(keys);
			if (r.ok) phase = 'done';
			else {
				phase = 'failed';
				failure = r.message;
			}
		} catch {
			phase = 'failed';
			failure = 'Could not save the repositories.';
		}
	}

	const items = $derived<StepItem[]>([
		{
			id: 'check',
			label: 'Check repositories against your GitHub list',
			status: phase === 'saving' ? 'active' : phase === 'failed' ? 'failed' : 'done',
			message: phase === 'failed' ? failure : undefined
		},
		{
			id: 'save',
			label: 'Update the Project',
			status: phase === 'done' ? 'done' : 'pending'
		},
		{
			id: 'apply',
			label: 'New events and runs use the new list (no restart)',
			status: phase === 'done' ? 'done' : 'pending'
		}
	]);
</script>

<div class="ez-stack">
	{#if canEdit}
		{#if problem}<Notice tone="info">{problem} You can still remove repositories.</Notice>{/if}
		<ul class="ez-repo-list" aria-label="Repositories">
			{#each rows as r (r.key)}
				<li>
					<Checkbox
						label={r.label}
						disabled={!!r.holder || phase === 'saving'}
						help={r.holder
							? `In "${r.holder}"`
							: r.missing
								? 'No longer in your GitHub list (archived or access removed)'
								: undefined}
						bind:checked={
							() => selected.has(r.key), (v) => (v ? selected.add(r.key) : selected.delete(r.key))
						}
					/>
				</li>
			{/each}
		</ul>
		{#if keys.length === 0}<Notice tone="error">Select at least one repository.</Notice>{/if}
		<div>
			<Button
				variant="primary"
				disabled={!dirty || keys.length === 0 || phase === 'saving'}
				onclick={submit}
			>
				Save repositories
			</Button>
		</div>
		{#if phase !== 'idle'}<StepList {items} label="Repository change progress" />{/if}
	{:else}
		<ul class="ez-repo-list">
			{#each current as k (k)}<li>{k.replace(/^github:/, '')}</li>{/each}
		</ul>
		<p class="ez-muted">Only the project owner can change these settings.</p>
	{/if}
</div>
