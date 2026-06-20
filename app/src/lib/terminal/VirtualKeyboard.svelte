<script lang="ts">
	import { encode, layouts, type KeyDef, type KeyboardLayout, type ModName, type Mods } from './keyboard';

	let { send, layout = layouts[0] }: { send: (data: string) => void; layout?: KeyboardLayout } = $props();

	// Each modifier is a tri-state: off → armed for next key → locked → off.
	type ModState = 'off' | 'once' | 'lock';
	let modState = $state<Record<ModName, ModState>>({ ctrl: 'off', alt: 'off', shift: 'off' });
	let fnVisible = $state(false);

	const mods = $derived<Mods>({
		ctrl: modState.ctrl !== 'off',
		alt: modState.alt !== 'off',
		shift: modState.shift !== 'off'
	});

	function cycleMod(m: ModName) {
		modState[m] = modState[m] === 'off' ? 'once' : modState[m] === 'once' ? 'lock' : 'off';
	}

	// Clear modifiers that were only armed for a single key press.
	function consumeMods() {
		for (const m of ['ctrl', 'alt', 'shift'] as ModName[]) {
			if (modState[m] === 'once') modState[m] = 'off';
		}
	}

	function press(key: KeyDef) {
		if (key.mod) {
			cycleMod(key.mod);
			return;
		}
		if (key.action === '__fn') {
			fnVisible = !fnVisible;
			return;
		}
		const data = encode(key, mods);
		if (data) send(data);
		consumeMods();
	}

	function modClass(key: KeyDef): string {
		if (!key.mod) return '';
		const s = modState[key.mod];
		return s === 'once' ? 'armed' : s === 'lock' ? 'locked' : '';
	}
</script>

<div class="vkb" role="group" aria-label="Virtual keyboard">
	{#if fnVisible}
		<div class="row fn-row">
			{#each layout.fnRow as key}
				<button
					class="key wide"
					style="flex-grow: {key.width ?? 1}"
					onpointerdown={(e) => { e.preventDefault(); press(key); }}
				>{key.label}</button>
			{/each}
		</div>
	{/if}
	{#each layout.rows as row}
		<div class="row">
			{#each row as key}
				<button
					class="key {key.wide ? 'wide' : ''} {modClass(key)}"
					style="flex-grow: {key.width ?? 1}"
					aria-pressed={key.mod ? modState[key.mod] !== 'off' : undefined}
					onpointerdown={(e) => { e.preventDefault(); press(key); }}
				>
					{#if key.shiftLabel && mods.shift}{key.shiftLabel}{:else}{key.label}{/if}
				</button>
			{/each}
		</div>
	{/each}
</div>

<style>
	.vkb {
		display: flex;
		flex-direction: column;
		gap: 3px;
		padding: 3px;
		background: #1a1a1e;
		border-top: 1px solid var(--color-border);
		user-select: none;
		-webkit-user-select: none;
		touch-action: manipulation;
	}
	.row {
		display: flex;
		gap: 3px;
	}
	.key {
		flex: 1 1 0;
		flex-basis: 0;
		min-width: 0;
		padding: 0;
		height: 38px;
		border: 1px solid #34343a;
		border-radius: 5px;
		background: #2a2a30;
		color: #e8e8ed;
		font-family: var(--font-mono), monospace;
		font-size: 14px;
		line-height: 1;
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		justify-content: center;
	}
	.key.wide {
		font-size: 11px;
		color: #b8b8c0;
	}
	.key:active {
		background: #3a3a42;
	}
	.key.armed {
		background: var(--color-accent);
		color: #fff;
		border-color: var(--color-accent);
	}
	.key.locked {
		background: var(--color-accent);
		color: #fff;
		border-color: #fff;
		box-shadow: inset 0 0 0 1px #fff;
	}
	.fn-row .key {
		height: 32px;
	}
</style>
