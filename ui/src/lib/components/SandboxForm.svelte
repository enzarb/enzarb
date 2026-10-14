<script lang="ts">
	import * as v from 'valibot';
	import {
		buildSandboxChangeSteps,
		buildStorageResizeStep,
		type StepState
	} from '#lib/conditions.ts';
	import type { ProjectView } from '#lib/project-types.ts';
	import { DEFAULT_MAX_RUNS, MaxRunsSchema } from '#lib/projects.ts';
	import {
		DEFAULT_STORAGE,
		MAX_CPU,
		MemorySchema,
		MIN_CPU,
		StorageSchema,
		parseCpuInput,
		parseQuantityInput,
		validateStorageGrowth,
		type SandboxResourcesInput
	} from '#lib/sandbox-resources.ts';
	import Button from './form/Button.svelte';
	import MaxRunsInput from './MaxRunsInput.svelte';
	import QuantityField from './QuantityField.svelte';
	import SandboxChangeNotice from './SandboxChangeNotice.svelte';
	import KeyValueList from './ui/KeyValueList.svelte';
	import Notice from './ui/Notice.svelte';
	import StepList, { type StepItem, type StepStatus } from './ui/StepList.svelte';

	/**
	 * The Project's sandbox settings: CPU, memory, storage (`spec.sandbox.*`) and concurrency
	 * (max runs). Read-only until Edit; one Save runs the two commands for whatever changed. Until
	 * Save, an inline notice warns when the edit restarts the sandbox after running jobs finish;
	 * afterwards the pending-change banner reports progress. Without `canEdit` it stays read-only.
	 */
	let {
		project,
		saveResources,
		saveMaxRuns,
		canEdit = true,
		tracking = $bindable(false)
	}: {
		project: ProjectView;
		saveResources: (input: SandboxResourcesInput) => Promise<void>;
		/** `null` stores "unset", so the Project follows the default. */
		saveMaxRuns: (maxRuns: number | null) => Promise<void>;
		canEdit?: boolean;
		/** True while the checklist still waits on the operator, so the page keeps polling. */
		tracking?: boolean;
	} = $props();

	const saved = $derived(project.sandboxSettings);
	const savedStorage = $derived(saved.storage ?? DEFAULT_STORAGE);
	const savedMaxRuns = $derived(project.maxRuns ?? DEFAULT_MAX_RUNS);

	let editing = $state(false);
	let cpuText = $state('');
	let memText = $state('');
	let stoText = $state('');
	let maxRuns = $state<number | null | ''>(DEFAULT_MAX_RUNS);

	function reset() {
		cpuText = saved.cpu === null ? '' : String(saved.cpu);
		memText = saved.memory ?? '';
		stoText = savedStorage;
		maxRuns = savedMaxRuns;
	}
	// Follow the stored spec while reading; an open edit keeps the user's draft.
	$effect(() => {
		void JSON.stringify([saved, savedMaxRuns]);
		if (!editing) reset();
	});

	function edit() {
		reset();
		error = '';
		editing = true;
	}
	function cancel() {
		reset();
		error = '';
		editing = false;
	}

	const cpu = $derived(parseCpuInput(cpuText));
	const memory = $derived.by(() => {
		if (memText.trim() === '') return { ok: true as const, value: null };
		const q = parseQuantityInput(memText);
		if (!q.ok) return q;
		const r = v.safeParse(MemorySchema, q.normalized);
		return r.success
			? { ok: true as const, value: q.normalized }
			: { ok: false as const, message: r.issues[0].message };
	});
	const storage = $derived.by(() => {
		if (stoText.trim() === '') return { ok: false as const, message: 'Enter a size such as 20Gi' };
		const q = parseQuantityInput(stoText);
		if (!q.ok) return q;
		const r = v.safeParse(StorageSchema, q.normalized);
		if (!r.success) return { ok: false as const, message: r.issues[0].message };
		const grow = validateStorageGrowth(saved.storage, q.normalized);
		return grow
			? { ok: false as const, message: grow }
			: { ok: true as const, value: q.normalized };
	});
	const maxRunsValid = $derived(v.safeParse(MaxRunsSchema, maxRuns).success);

	const cpuChanged = $derived(
		cpu.ok ? cpu.value !== saved.cpu : cpuText.trim() !== String(saved.cpu ?? '')
	);
	const memChanged = $derived(
		memory.ok ? memory.value !== saved.memory : memText.trim() !== (saved.memory ?? '')
	);
	const stoChanged = $derived(
		storage.ok ? storage.value !== savedStorage : stoText.trim() !== savedStorage
	);
	const maxChanged = $derived(maxRuns !== savedMaxRuns);
	const resourcesChanged = $derived(cpuChanged || memChanged || stoChanged);

	const dirty = $derived(resourcesChanged || maxChanged);
	const valid = $derived(cpu.ok && memory.ok && storage.ok && maxRunsValid);

	let saving = $state(false);
	let shown = $state(false);
	let error = $state('');

	async function submit() {
		if (!cpu.ok || !memory.ok || !storage.ok || typeof maxRuns !== 'number') return;
		error = '';
		saving = true;
		const sendMax = maxChanged;
		const sendResources = resourcesChanged;
		let maxSaved = false;
		try {
			if (sendMax) {
				try {
					await saveMaxRuns(maxRuns === DEFAULT_MAX_RUNS ? null : maxRuns);
				} catch {
					error = 'Could not save the concurrency setting.';
					return;
				}
				maxSaved = true;
			}
			if (sendResources) {
				shown = true;
				tracking = true;
				try {
					await saveResources({
						cpu: cpu.value,
						memory: memory.value,
						// Unchanged storage is not sent, so it is never reset.
						storage: stoChanged ? storage.value : undefined
					});
				} catch {
					shown = false;
					tracking = false;
					error = maxSaved
						? 'Concurrency saved, but the sandbox resources could not be saved.'
						: 'Could not save the sandbox resources.';
					return;
				}
			}
			editing = false;
		} finally {
			saving = false;
		}
	}

	const steps = $derived(
		[
			...buildSandboxChangeSteps(project, saving),
			buildStorageResizeStep(project.conditions)
		].filter((s) => s !== null)
	);
	// Stop polling once the change is applied (or failed); the checklist stays for reference.
	$effect(() => {
		if (!tracking || saving) return;
		const last = steps.find((s) => s.id === 'ready');
		const failed = steps.some((s) => s.state === 'failed');
		if (last?.state === 'done' || failed) tracking = false;
	});

	const status: Record<StepState, StepStatus> = {
		pending: 'pending',
		'in-progress': 'active',
		done: 'done',
		failed: 'failed'
	};
	const words: Record<StepState, string> = {
		pending: 'Pending',
		'in-progress': 'In progress',
		done: 'Done',
		failed: 'Failed'
	};
	const items = $derived<StepItem[]>(
		steps.map((s) => ({
			id: s.id,
			status: status[s.state],
			label: s.label,
			stateText: words[s.state],
			message: s.message || undefined,
			attrs: { 'data-state': s.state }
		}))
	);

	const effective = $derived([
		{ term: 'CPU', value: saved.cpu === null ? 'not set (no limit)' : `${saved.cpu}` },
		{ term: 'Memory', value: saved.memory ?? 'not set (no limit)' },
		{
			term: 'Storage',
			value: saved.storage ? saved.storage : `${DEFAULT_STORAGE} (default)`
		},
		{
			term: 'Concurrent runs',
			value: project.maxRuns === undefined ? `${DEFAULT_MAX_RUNS} (default)` : `${project.maxRuns}`
		}
	]);

	const cpuHelp = `Whole cores, ${MIN_CPU} to ${MAX_CPU}, e.g. 8. Leave empty for no limit.`;
	const memHelp = 'e.g. 512Mi or 16Gi, 512Mi to 256Gi. Leave empty for no limit.';
	const stoHelp = 'e.g. 20Gi, up to 256Gi. Can only grow.';
	const maxNote = `Saving the default (${DEFAULT_MAX_RUNS}) clears the setting, so the Project follows the default.`;
</script>

<div class="ez-sandbox-resources">
	{#if canEdit && editing}
		<div class="ez-settings">
			<QuantityField
				label="CPU (cores)"
				bind:value={cpuText}
				help={cpuHelp}
				issues={cpu.ok ? [] : [cpu.message]}
				placeholder="no limit"
			/>
			<QuantityField
				label="Memory"
				bind:value={memText}
				help={memHelp}
				issues={memory.ok ? [] : [memory.message]}
				placeholder="no limit"
			/>
			<QuantityField
				label="Storage"
				bind:value={stoText}
				help={stoHelp}
				issues={storage.ok ? [] : [storage.message]}
			/>
			<MaxRunsInput bind:value={maxRuns} note={maxNote} />
		</div>
		<SandboxChangeNotice visible={dirty && !saving} />
		<div>
			<Button variant="primary" disabled={!valid || !dirty || saving} onclick={submit}>
				{saving ? 'Saving…' : 'Save sandbox'}
			</Button>
			<Button variant="ghost" disabled={saving} onclick={cancel}>Cancel</Button>
		</div>
	{:else}
		<KeyValueList items={effective} />
		{#if canEdit}
			<div><Button onclick={edit}>Edit sandbox</Button></div>
		{:else}
			<p class="ez-muted">Only the project owner can change these settings.</p>
		{/if}
	{/if}

	{#if error}<Notice tone="error">{error}</Notice>{/if}
	{#if shown || steps.some((s) => s.id === 'storage')}
		<StepList {items} label="Sandbox change progress" />
	{/if}
</div>
