<script lang="ts">
	import * as v from 'valibot';
	import Field from './form/Field.svelte';
	import { describedBy, type Issues } from './form/util.ts';
	import { DEFAULT_MAX_RUNS, MAX_MAX_RUNS, MIN_MAX_RUNS, MaxRunsSchema } from '#lib/projects.ts';

	let {
		value = $bindable(DEFAULT_MAX_RUNS),
		issues,
		label = 'Concurrent runs',
		allowEmpty = false,
		note = ''
	}: {
		value?: number | null | '';
		issues?: Issues;
		label?: string;
		/** Empty input means "use the default" (create form) instead of being an error. */
		allowEmpty?: boolean;
		/** Extra help text appended to the default/range hint. */
		note?: string;
	} = $props();

	const uid = $props.id();
	const help = $derived(
		`Default: ${DEFAULT_MAX_RUNS}. Whole number from ${MIN_MAX_RUNS} to ${MAX_MAX_RUNS}.${note ? ` ${note}` : ''}`
	);
	const empty = $derived(value === null || value === '');
	const own = $derived.by(() => {
		if (empty) return allowEmpty ? [] : ['Enter a whole number'];
		const r = v.safeParse(MaxRunsSchema, value);
		return r.success ? [] : [r.issues[0].message];
	});
	const shown = $derived([...own, ...(issues ?? [])]);
</script>

<Field id={uid} {label} {help} issues={shown}>
	<input
		class="ez-control ez-max-runs"
		id={uid}
		type="number"
		min={MIN_MAX_RUNS}
		max={MAX_MAX_RUNS}
		step="1"
		bind:value
		aria-invalid={shown.length > 0 ? 'true' : undefined}
		aria-describedby={describedBy(uid, help, shown)}
	/>
</Field>
