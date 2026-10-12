<script lang="ts">
	import * as v from 'valibot';
	import {
		buildSandboxChangeSteps,
		buildStorageResizeStep,
		type StepState
	} from '#lib/conditions.ts';
	import { POD_SHAPE_CONFIRM_COPY, POD_SHAPE_QUEUED_COPY } from '#lib/pending-change.ts';
	import type { ProjectView } from '#lib/project-types.ts';
	import {
		CpuSchema,
		DEFAULT_STORAGE,
		MAX_CPU,
		MemorySchema,
		MIN_CPU,
		StorageSchema,
		joinQuantity,
		splitQuantity,
		validateStorageGrowth,
		type QuantityUnit,
		type SandboxResourcesInput
	} from '#lib/sandbox-resources.ts';
	import Button from './form/Button.svelte';
	import Field from './form/Field.svelte';
	import { describedBy } from './form/util.ts';
	import ConfirmDialog from './ui/ConfirmDialog.svelte';
	import KeyValueList from './ui/KeyValueList.svelte';
	import Notice from './ui/Notice.svelte';
	import StepList, { type StepItem, type StepStatus } from './ui/StepList.svelte';

	/**
	 * Sandbox CPU, memory and storage (`spec.sandbox.{cpu,memory,storage}`). Saving is confirmed
	 * first: the operator queues the change until running jobs finish. Without `canEdit` the
	 * current values are shown read-only.
	 */
	let {
		project,
		save,
		canEdit = true,
		tracking = $bindable(false)
	}: {
		project: ProjectView;
		save: (input: SandboxResourcesInput) => Promise<void>;
		canEdit?: boolean;
		/** True while the checklist still waits on the operator, so the page keeps polling. */
		tracking?: boolean;
	} = $props();

	const uid = $props.id();
	const units: QuantityUnit[] = ['Mi', 'Gi'];

	const saved = $derived(project.sandboxSettings);
	const savedKey = $derived(JSON.stringify(saved));

	let cpu = $state<number | null>(null);
	let memValue = $state('');
	let memUnit = $state<QuantityUnit>('Gi');
	let stoValue = $state('');
	let stoUnit = $state<QuantityUnit>('Gi');
	// Re-seed the draft whenever the stored spec changes (initially, and after a save lands).
	$effect(() => {
		void savedKey;
		cpu = saved.cpu;
		({ value: memValue, unit: memUnit } = splitQuantity(saved.memory));
		({ value: stoValue, unit: stoUnit } = splitQuantity(saved.storage ?? DEFAULT_STORAGE));
	});

	const memory = $derived(joinQuantity(memValue, memUnit));
	const storage = $derived(joinQuantity(stoValue, stoUnit));

	const cpuIssues = $derived.by(() => {
		if (cpu === null || (cpu as unknown) === '') return [];
		const r = v.safeParse(CpuSchema, cpu);
		return r.success ? [] : [r.issues[0].message];
	});
	const memoryIssues = $derived.by(() => {
		if (memory === null) return [];
		const r = v.safeParse(MemorySchema, memory);
		return r.success ? [] : [r.issues[0].message];
	});
	const storageIssues = $derived.by(() => {
		if (storage === null) return ['Enter a size such as 20Gi'];
		const r = v.safeParse(StorageSchema, storage);
		if (!r.success) return [r.issues[0].message];
		const grow = validateStorageGrowth(saved.storage, storage);
		return grow ? [grow] : [];
	});

	const cpuValue = $derived((cpu as unknown) === '' ? null : cpu);
	const dirty = $derived(
		cpuValue !== saved.cpu ||
			memory !== saved.memory ||
			storage !== (saved.storage ?? DEFAULT_STORAGE)
	);
	const valid = $derived(
		cpuIssues.length === 0 && memoryIssues.length === 0 && storageIssues.length === 0
	);

	let confirming = $state(false);
	let saving = $state(false);
	let shown = $state(false);
	let error = $state('');

	async function confirm() {
		error = '';
		saving = true;
		shown = true;
		tracking = true;
		try {
			await save({
				cpu: cpuValue,
				memory,
				// Unchanged storage is not sent, so it is never reset.
				storage: storage !== (saved.storage ?? DEFAULT_STORAGE) ? storage : undefined
			});
		} catch {
			error = 'Could not save the sandbox resources.';
			shown = false;
			tracking = false;
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
		}
	]);

	const cpuHelp = `Whole cores, ${MIN_CPU} to ${MAX_CPU}. Leave empty for no limit.`;
	const memHelp = 'Mi or Gi, 512Mi to 256Gi. Leave empty for no limit.';
	const stoHelp = 'Mi or Gi, up to 256Gi. Can only grow.';
</script>

<div class="ez-sandbox-resources">
	<KeyValueList items={effective} />

	{#if canEdit}
		<div class="ez-settings">
			<Field id="{uid}-cpu" label="CPU (cores)" help={cpuHelp} issues={cpuIssues}>
				<input
					class="ez-control ez-quantity"
					id="{uid}-cpu"
					type="number"
					min={MIN_CPU}
					max={MAX_CPU}
					step="1"
					placeholder="no limit"
					bind:value={cpu}
					aria-invalid={cpuIssues.length > 0 ? 'true' : undefined}
					aria-describedby={describedBy(`${uid}-cpu`, cpuHelp, cpuIssues)}
				/>
			</Field>
			<Field id="{uid}-mem" label="Memory" help={memHelp} issues={memoryIssues}>
				<div class="ez-cluster">
					<input
						class="ez-control ez-quantity"
						id="{uid}-mem"
						type="text"
						inputmode="numeric"
						placeholder="no limit"
						bind:value={memValue}
						aria-invalid={memoryIssues.length > 0 ? 'true' : undefined}
						aria-describedby={describedBy(`${uid}-mem`, memHelp, memoryIssues)}
					/>
					<select class="ez-control" aria-label="Memory unit" bind:value={memUnit}>
						{#each units as u (u)}<option value={u}>{u}</option>{/each}
					</select>
				</div>
			</Field>
			<Field id="{uid}-sto" label="Storage" help={stoHelp} issues={storageIssues}>
				<div class="ez-cluster">
					<input
						class="ez-control ez-quantity"
						id="{uid}-sto"
						type="text"
						inputmode="numeric"
						bind:value={stoValue}
						aria-invalid={storageIssues.length > 0 ? 'true' : undefined}
						aria-describedby={describedBy(`${uid}-sto`, stoHelp, storageIssues)}
					/>
					<select class="ez-control" aria-label="Storage unit" bind:value={stoUnit}>
						{#each units as u (u)}<option value={u}>{u}</option>{/each}
					</select>
				</div>
			</Field>
		</div>
		<div>
			<Button
				variant="primary"
				disabled={!valid || !dirty || saving}
				onclick={() => (confirming = true)}
			>
				Save resources
			</Button>
		</div>
	{:else}
		<p class="ez-muted">Only the project owner can change these settings.</p>
	{/if}

	{#if error}<Notice tone="error">{error}</Notice>{/if}
	{#if shown || steps.some((s) => s.id === 'storage')}
		{#if shown && !saving}<Notice tone="info">{POD_SHAPE_QUEUED_COPY}</Notice>{/if}
		<StepList {items} label="Sandbox change progress" />
	{/if}
</div>

<ConfirmDialog
	bind:open={confirming}
	title="Apply sandbox resource changes?"
	description={POD_SHAPE_CONFIRM_COPY}
	confirmLabel="Save changes"
	tone="primary"
	onconfirm={confirm}
/>
