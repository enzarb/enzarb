import { tick, untrack } from 'svelte';

interface SyncOptions {
	/** Field name, if the component posts anything. */
	name: () => string | undefined;
	/** Current value; any change dispatches an `input` event so SvelteKit remote forms see it. */
	value: () => unknown;
	/** Restore the initial value (the form was reset). */
	reset: () => void;
	/** Impersonate a checkbox when no hidden input exists (unchecked => value null). */
	checkbox?: boolean;
}

/**
 * Hidden inputs never fire `input` events when state changes, so remote forms would never
 * mark the field dirty, update `fields.x.value()` or clear its issue. This dispatches one from
 * the hidden input named `name` after each change. When no such input is rendered (empty
 * multi-select, unchecked box, unselected radio) it dispatches from an invisible stand-in that
 * carries `name` as a property only, which is all SvelteKit's handler reads for those cases.
 * It also restores the initial value when the surrounding form is reset.
 *
 * Returns an attachment: put `{@attach sync}` on the component's root element.
 */
export function formSync(opts: SyncOptions) {
	let root: HTMLElement | undefined;
	let stand: HTMLElement | undefined;
	let first = true;
	let suppress = false;
	let last = '';
	const serialized = () => JSON.stringify(opts.value() ?? null);

	function target(name: string): HTMLElement {
		const real = [...root!.querySelectorAll('input')].find((i) => i.name === name);
		if (real) return real;
		if (!stand) {
			stand = document.createElement('span');
			stand.hidden = true;
			Object.defineProperty(stand, 'name', { get: () => opts.name() });
			// Array names (`x[]/id`): kit filters checkbox-typed inputs by `.checked`, which would
			// drop the real hidden inputs' values, so only single-valued boxes impersonate one.
			if (opts.checkbox && !opts.name()?.includes('[]/')) {
				Object.defineProperty(stand, 'type', { value: 'checkbox' });
				Object.defineProperty(stand, 'checked', { value: false });
			}
			root!.append(stand);
		}
		return stand;
	}

	$effect(() => {
		const now = serialized(); // track
		if (first) {
			first = false;
			last = now;
			return;
		}
		if (now === last) return; // same content in a new array
		last = now;
		if (suppress) {
			// the change came from a form reset; kit clears `dirty` itself then
			suppress = false;
			return;
		}
		untrack(() => {
			void tick().then(() => {
				if (!root?.isConnected) return;
				const name = opts.name();
				if (name) target(name).dispatchEvent(new Event('input', { bubbles: true }));
			});
		});
	});

	function onreset() {
		const before = serialized();
		opts.reset();
		suppress = serialized() !== before;
	}

	return (node: HTMLElement) => {
		root = node;
		const form = node.closest('form');
		form?.addEventListener('reset', onreset);
		return () => form?.removeEventListener('reset', onreset);
	};
}
