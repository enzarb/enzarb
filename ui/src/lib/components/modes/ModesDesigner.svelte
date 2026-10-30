<script lang="ts">
	import { untrack } from 'svelte';
	import Button from '../form/Button.svelte';
	import TextArea from '../form/TextArea.svelte';
	import TextField from '../form/TextField.svelte';
	import Notice from '../ui/Notice.svelte';
	import Section from '../ui/Section.svelte';
	import Tabs from '../ui/Tabs.svelte';
	import type { StepItem } from '../ui/StepList.svelte';
	import IssueList from './IssueList.svelte';
	import ModeForm from './ModeForm.svelte';
	import ModeList from './ModeList.svelte';
	import SaveSteps from './SaveSteps.svelte';
	import {
		blankMode,
		freeModeId,
		hasErrors,
		removeMode,
		renameMode,
		type ModeDto,
		type ModeIssue,
		type ModesDoc,
		type ModesFile
	} from '#lib/modes-types.ts';

	type Fail = { ok: false; message: string; issues?: ModeIssue[]; code?: string };
	export type DesignerApi = {
		validate: (doc: ModesDoc) => Promise<{ ok: true; issues: ModeIssue[] } | Fail>;
		parse: (text: string) => Promise<{ ok: true; doc: ModesDoc; issues: ModeIssue[] } | Fail>;
		render: (doc: ModesDoc, old: string | null) => Promise<{ ok: true; text: string } | Fail>;
		save: (input: {
			doc?: ModesDoc;
			text?: string;
			message: string;
		}) => Promise<{ ok: true; unchanged: boolean; commitUrl: string | null } | Fail>;
		reload: () => void;
	};

	/**
	 * The modes designer for one repository's `.ez/modes.toml`: a form per mode and a raw TOML
	 * editor, validated by tracks as the user edits; Save commits to the default branch.
	 */
	let {
		file,
		api,
		saved
	}: {
		file: ModesFile;
		api: DesignerApi;
		saved: (r: { unchanged: boolean; commitUrl: string | null }) => void;
	} = $props();

	const initial = untrack(() => file);
	const VALIDATE_DEBOUNCE_MS = 400;

	let doc = $state<ModesDoc>(structuredClone(initial.doc));
	let issues = $state<ModeIssue[]>(initial.issues);
	let selected = $state(0);
	let tab = $state(initial.parseError ? 'toml' : 'form');
	let text = $state(initial.text ?? '');
	let switchError = $state('');
	let switching = $state(false);
	let message = $state('');
	let steps = $state<StepItem[]>([]);
	let saving = $state(false);
	let conflict = $state(false);
	let saveTried = $state(false);

	const savedDoc = JSON.stringify(initial.doc);
	// Something to commit: the form differs from the saved modes, or the TOML from the saved text.
	const dirty = $derived(
		tab === 'form' ? JSON.stringify(doc) !== savedDoc : text !== (initial.text ?? '')
	);
	// A repo with no modes file is not an error until the user edits something or picks a template.
	const shownIssues = $derived(initial.text !== null || dirty || saveTried ? issues : []);
	const blocked = $derived(tab === 'form' && (hasErrors(issues) || !doc.modes.length));
	const ids = $derived(doc.modes.map((m) => m.id));
	const templateSets = $derived([...new Set(initial.catalog.templates.map((t) => t.set))]);
	const repoName = initial.repo.replace(/^github:/, '');

	// Validate the form as it changes (debounced; a newer edit wins).
	let seq = 0;
	$effect(() => {
		if (tab !== 'form') return;
		const snapshot = $state.snapshot(doc) as ModesDoc;
		const mine = ++seq;
		const id = setTimeout(async () => {
			const r = await api.validate(snapshot).catch(() => null);
			if (r?.ok && mine === seq) issues = r.issues;
		}, VALIDATE_DEBOUNCE_MS);
		return () => clearTimeout(id);
	});

	async function switchTab(next: string) {
		if (next === tab || switching) return;
		switchError = '';
		switching = true;
		try {
			if (next === 'toml') {
				const r = await api.render($state.snapshot(doc) as ModesDoc, initial.text);
				if (!r.ok) {
					switchError = r.issues?.length
						? 'Fix the errors in the form first: only valid modes can be written as TOML.'
						: r.message;
					if (r.issues) issues = r.issues;
					return;
				}
				text = r.text;
			} else {
				const r = await api.parse(text);
				if (!r.ok) {
					switchError = `The TOML cannot be opened in the form: ${r.message}`;
					return;
				}
				doc = r.doc;
				issues = r.issues;
				selected = Math.min(selected, Math.max(0, r.doc.modes.length - 1));
			}
			tab = next;
		} catch {
			switchError = 'The modes service did not answer. Try again.';
		} finally {
			switching = false;
		}
	}

	function add(from: ModeDto | null) {
		const base = from ? structuredClone($state.snapshot(from) as ModeDto) : blankMode('');
		delete base.originalId;
		base.id = freeModeId(doc, from?.id ?? 'mode');
		doc.modes.push(base);
		if (!doc.defaultMode) doc.defaultMode = base.id;
		selected = doc.modes.length - 1;
	}

	function startFrom(set: string | null) {
		const modes = set
			? initial.catalog.templates.filter((t) => t.set === set).map((t) => structuredClone(t.mode))
			: [blankMode('default')];
		doc = { defaultMode: modes[0]?.id ?? '', modes };
		selected = 0;
	}

	function setStep(id: string, status: StepItem['status'], extra: Partial<StepItem> = {}) {
		steps = steps.map((s) => (s.id === id ? { ...s, status, ...extra } : s));
	}

	async function save() {
		saving = true;
		saveTried = true;
		conflict = false;
		steps = [
			{ id: 'validate', status: 'active', label: 'Validate the modes' },
			{ id: 'commit', status: 'pending', label: `Commit ${initial.path} to ${initial.branch}` }
		];
		try {
			const fromForm = tab === 'form';
			const snapshot = $state.snapshot(doc) as ModesDoc;
			const check = fromForm ? await api.validate(snapshot) : await api.parse(text);
			if (!check.ok || hasErrors(check.issues)) {
				if (check.ok) issues = check.issues;
				else if (check.issues) issues = check.issues;
				setStep('validate', 'failed', {
					message: check.ok ? 'Fix the errors listed below.' : check.message
				});
				return;
			}
			issues = check.issues;
			setStep('validate', 'done');
			setStep('commit', 'active');
			const r = await api.save(fromForm ? { doc: snapshot, message } : { text, message });
			if (!r.ok) {
				conflict = r.code === 'conflict';
				if (r.issues) issues = r.issues;
				setStep('commit', 'failed', { message: r.message });
				return;
			}
			setStep('commit', 'done', {
				message: r.unchanged ? 'Nothing changed: no commit was made.' : undefined
			});
			saved({ unchanged: r.unchanged, commitUrl: r.commitUrl });
		} catch {
			steps = steps.map((s) =>
				s.status === 'active'
					? { ...s, status: 'failed', message: 'The request failed. Try again.' }
					: s
			);
		} finally {
			saving = false;
		}
	}
</script>

<div class="ez-stack">
	<p class="ez-muted">
		<span class="ez-mono">{initial.path}</span> on <span class="ez-mono">{repoName}</span>, branch
		<span class="ez-mono">{initial.branch}</span>. Saving commits straight to that branch.
	</p>

	{#if initial.parseError}
		<Notice tone="warning" title="The file is not valid modes TOML">
			{initial.parseError}. Fix it in the TOML tab, then switch to the form.
		</Notice>
	{/if}

	{#if tab === 'form' && !doc.modes.length}
		<Section title="Start your modes">
			<p class="ez-muted">
				{initial.text === null
					? `There is no ${initial.path} on ${initial.branch} yet.`
					: 'The file defines no modes.'}
				Start from one of ez's templates or a single blank mode.
			</p>
			<div class="ez-cluster">
				{#each templateSets as set (set)}
					<Button onclick={() => startFrom(set)}
						>{set === 'full' ? 'Full lifecycle (6 modes)' : `${set} template`}</Button
					>
				{/each}
				<Button onclick={() => startFrom(null)}>Blank mode</Button>
			</div>
		</Section>
	{/if}

	<Tabs
		label="Editor"
		tabs={[
			{ value: 'form', label: 'Form', disabled: switching },
			{ value: 'toml', label: 'TOML', disabled: switching }
		]}
		bind:value={() => tab, (v) => void switchTab(v)}
	>
		{#snippet panel(value)}
			{#if switchError}<Notice tone="error">{switchError}</Notice>{/if}
			{#if value === 'form' && doc.modes.length}
				<div class="ez-modes-layout">
					<ModeList
						modes={doc.modes}
						defaultMode={doc.defaultMode}
						bind:selected
						{issues}
						templates={initial.catalog.templates}
						{add}
					/>
					{#if doc.modes[selected]}
						{#key selected}
							<ModeForm
								bind:mode={doc.modes[selected]}
								index={selected}
								others={ids.filter((_, i) => i !== selected)}
								isDefault={doc.modes[selected].id === doc.defaultMode}
								catalog={initial.catalog}
								{issues}
								rename={(id) => (doc = renameMode(doc, selected, id))}
								remove={() => {
									doc = removeMode(doc, selected);
									selected = Math.max(0, selected - 1);
								}}
								makeDefault={() => (doc.defaultMode = doc.modes[selected].id)}
							/>
						{/key}
					{/if}
				</div>
			{:else if value === 'toml'}
				<TextArea
					label={initial.path}
					class="ez-control ez-mono"
					rows={24}
					spellcheck={false}
					bind:value={text}
				/>
			{/if}
		{/snippet}
	</Tabs>

	<Section title="Validation">
		<IssueList
			issues={shownIssues}
			select={(i) => {
				if (tab === 'form') selected = i;
			}}
		/>
	</Section>

	<Section title="Save">
		<div class="ez-stack">
			<TextField
				label="Commit message"
				placeholder="chore(ez): update .ez/modes.toml from enzarb"
				maxlength={200}
				bind:value={message}
				disabled={saving}
			/>
			<div class="ez-cluster">
				<Button
					variant="primary"
					disabled={!dirty || blocked || saving || switching}
					onclick={save}
				>
					{saving ? 'Saving…' : `Commit to ${initial.branch}`}
				</Button>
				{#if conflict}<Button onclick={api.reload}>Reload the file</Button>{/if}
			</div>
			<SaveSteps items={steps} />
		</div>
	</Section>
</div>
