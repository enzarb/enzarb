<script lang="ts">
	import Button from '../form/Button.svelte';
	import FieldSet from '../form/FieldSet.svelte';
	import Select from '../form/Select.svelte';
	import TextField from '../form/TextField.svelte';
	import type { HookDto, ModeIssue, ModesCatalog } from '#lib/modes-types.ts';
	import { issuesAt } from '#lib/modes-types.ts';

	/** Hooks per event; `base` is the issue path of this mode's hooks (`modes[2].hooks`). */
	let {
		hooks = $bindable({}),
		catalog,
		issues,
		base
	}: {
		hooks?: Record<string, HookDto[]>;
		catalog: ModesCatalog;
		issues: readonly ModeIssue[];
		base: string;
	} = $props();

	const failureOptions = $derived(catalog.onFailureValues.map((v) => ({ value: v, label: v })));
	const num = (s: string, min = 0) => Math.max(min, Math.trunc(Number(s)) || 0);

	function add(event: string) {
		const list = hooks[event] ?? [];
		hooks[event] = [
			...list,
			{ run: '', glob: null, matcher: null, onFailure: 'ignore', maxFixTurns: 3, timeoutSecs: 60 }
		];
	}
	function remove(event: string, i: number) {
		const list = (hooks[event] ?? []).filter((_, n) => n !== i);
		if (list.length) hooks[event] = list;
		else delete hooks[event];
	}
</script>

<FieldSet legend="Hooks">
	{#each catalog.hookEvents as ev (ev.name)}
		<div class="ez-stack ez-modes-hook-event">
			<div>
				<strong class="ez-mono">{ev.name}</strong>
				<span class="ez-muted">{ev.description}</span>
			</div>
			{#each hooks[ev.name] ?? [] as hook, i (i)}
				{@const at = `${base}.${ev.name}[${i}]`}
				<div class="ez-stack ez-modes-hook">
					<TextField
						label="Command"
						class="ez-control ez-mono"
						bind:value={hook.run}
						issues={issuesAt(issues, `${at}.run`)}
					/>
					<div class="ez-cluster ez-cluster--end">
						{#if ev.glob}
							<TextField
								label="Files (glob)"
								placeholder="**/*.rs"
								bind:value={() => hook.glob ?? '', (v) => (hook.glob = v || null)}
								issues={issuesAt(issues, `${at}.glob`)}
							/>
						{/if}
						{#if ev.matcher}
							<TextField
								label="Tools (matcher)"
								placeholder="Bash"
								bind:value={() => hook.matcher ?? '', (v) => (hook.matcher = v || null)}
								issues={issuesAt(issues, `${at}.matcher`)}
							/>
						{/if}
						<Select label="On failure" options={failureOptions} bind:value={hook.onFailure} />
						{#if hook.onFailure === 'fix_turn'}
							<TextField
								label="Fix turns"
								type="number"
								min="1"
								bind:value={() => String(hook.maxFixTurns), (v) => (hook.maxFixTurns = num(v, 1))}
							/>
						{/if}
						<TextField
							label="Timeout (s)"
							type="number"
							min="1"
							bind:value={() => String(hook.timeoutSecs), (v) => (hook.timeoutSecs = num(v, 1))}
						/>
						<Button variant="ghost" onclick={() => remove(ev.name, i)}>Remove</Button>
					</div>
				</div>
			{/each}
			<div><Button size="sm" onclick={() => add(ev.name)}>Add {ev.name} hook</Button></div>
		</div>
	{/each}
</FieldSet>
