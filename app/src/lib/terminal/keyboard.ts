// On-screen virtual keyboard for the mobile terminal.
//
// The OS keyboard can't produce Ctrl/Alt/Fn or reliable escape sequences, so on
// touch devices we suppress it and render our own. This module owns the layout
// data and the translation from a key press (+ active modifiers) into the bytes
// a terminal expects.

export type ModName = 'ctrl' | 'alt' | 'shift';

export interface KeyDef {
	/** Glyph shown on the key (unshifted). */
	label: string;
	/** Glyph shown above/instead when Shift is active. */
	shiftLabel?: string;
	/** Relative width; 1 = one standard key. */
	width?: number;
	/** Visual emphasis for non-character keys. */
	wide?: boolean;
	/** Printable key: unshifted output. */
	code?: string;
	/** Printable key: shifted output. */
	shift?: string;
	/** Sticky modifier key. */
	mod?: ModName;
	/** Named special key (see encode()). */
	action?: string;
}

export interface Mods {
	ctrl: boolean;
	alt: boolean;
	shift: boolean;
}

export interface KeyboardLayout {
	name: string;
	/** The main grid. */
	rows: KeyDef[][];
	/** Function-key row, shown when the Fn toggle is on. */
	fnRow: KeyDef[];
}

const CSI = '\x1b[';
const SS3 = '\x1bO';

// xterm modifier encoding: 1 + shift(1) + alt(2) + ctrl(4).
function modParam(m: Mods): number {
	let v = 0;
	if (m.shift) v += 1;
	if (m.alt) v += 2;
	if (m.ctrl) v += 4;
	return v ? 1 + v : 0;
}

// A cursor / editing key applies modifiers via the `CSI 1 ; <p> <final>` form,
// falling back to the bare sequence when no modifier is held.
function csiKey(final: string, m: Mods): string {
	const p = modParam(m);
	return p ? `${CSI}1;${p}${final}` : `${CSI}${final}`;
}

function tildeKey(num: number, m: Mods): string {
	const p = modParam(m);
	return p ? `${CSI}${num};${p}~` : `${CSI}${num}~`;
}

// Map a single character to its Ctrl-modified control code.
function controlChar(ch: string): string | null {
	if (ch.length !== 1) return null;
	const c = ch.toLowerCase();
	if (c >= 'a' && c <= 'z') return String.fromCharCode(c.charCodeAt(0) - 96);
	const specials: Record<string, number> = {
		' ': 0, '@': 0, '[': 27, '\\': 28, ']': 29, '^': 30, '_': 31, '?': 127, '/': 31
	};
	if (ch in specials) return String.fromCharCode(specials[ch]);
	return null;
}

const FN_TILDE: Record<string, number> = {
	f5: 15, f6: 17, f7: 18, f8: 19, f9: 20, f10: 21, f11: 23, f12: 24
};
const FN_SS3: Record<string, string> = { f1: 'P', f2: 'Q', f3: 'R', f4: 'S' };

/**
 * Translate a pressed key + the currently active modifiers into the byte
 * string to send to the PTY. Returns '' for keys with no output (bare
 * modifier presses are handled by the caller, not here).
 */
export function encode(key: KeyDef, mods: Mods): string {
	if (key.mod) return '';

	if (key.action) {
		const a = key.action;
		switch (a) {
			case 'enter':
				return '\r';
			case 'tab':
				return '\t';
			case 'backspace':
				return '\x7f';
			case 'esc':
				return '\x1b';
			case 'space': {
				if (mods.ctrl) return '\x00';
				const base = ' ';
				return mods.alt ? '\x1b' + base : base;
			}
			case 'up':
				return csiKey('A', mods);
			case 'down':
				return csiKey('B', mods);
			case 'right':
				return csiKey('C', mods);
			case 'left':
				return csiKey('D', mods);
			case 'home':
				return csiKey('H', mods);
			case 'end':
				return csiKey('F', mods);
			case 'insert':
				return tildeKey(2, mods);
			case 'delete':
				return tildeKey(3, mods);
			case 'pageup':
				return tildeKey(5, mods);
			case 'pagedown':
				return tildeKey(6, mods);
		}
		if (a in FN_SS3) {
			const p = modParam(mods);
			return p ? `${CSI}1;${p}${FN_SS3[a]}` : `${SS3}${FN_SS3[a]}`;
		}
		if (a in FN_TILDE) return tildeKey(FN_TILDE[a], mods);
		return '';
	}

	// Printable character key.
	let ch = mods.shift ? key.shift ?? key.code ?? '' : key.code ?? '';
	if (!ch) return '';
	if (mods.ctrl) {
		const c = controlChar(ch);
		if (c !== null) ch = c;
	}
	if (mods.alt) ch = '\x1b' + ch;
	return ch;
}

// --- Layouts -------------------------------------------------------------

function c(code: string, shift: string): KeyDef {
	return { label: code, shiftLabel: shift, code, shift };
}
// Letter: label shows uppercase, output respects Shift.
function l(letter: string): KeyDef {
	return { label: letter.toUpperCase(), code: letter, shift: letter.toUpperCase() };
}

export const usQwerty: KeyboardLayout = {
	name: 'US',
	fnRow: [
		{ label: 'F1', action: 'f1', wide: true },
		{ label: 'F2', action: 'f2', wide: true },
		{ label: 'F3', action: 'f3', wide: true },
		{ label: 'F4', action: 'f4', wide: true },
		{ label: 'F5', action: 'f5', wide: true },
		{ label: 'F6', action: 'f6', wide: true },
		{ label: 'F7', action: 'f7', wide: true },
		{ label: 'F8', action: 'f8', wide: true },
		{ label: 'F9', action: 'f9', wide: true },
		{ label: 'F10', action: 'f10', wide: true },
		{ label: 'F11', action: 'f11', wide: true },
		{ label: 'F12', action: 'f12', wide: true }
	],
	rows: [
		[
			c('`', '~'), c('1', '!'), c('2', '@'), c('3', '#'), c('4', '$'), c('5', '%'),
			c('6', '^'), c('7', '&'), c('8', '*'), c('9', '('), c('0', ')'), c('-', '_'), c('=', '+'),
			{ label: '⌫', action: 'backspace', width: 1.5, wide: true }
		],
		[
			{ label: 'Tab', action: 'tab', width: 1.5, wide: true },
			l('q'), l('w'), l('e'), l('r'), l('t'), l('y'), l('u'), l('i'), l('o'), l('p'),
			c('[', '{'), c(']', '}'), c('\\', '|')
		],
		[
			{ label: 'Esc', action: 'esc', width: 1.75, wide: true },
			l('a'), l('s'), l('d'), l('f'), l('g'), l('h'), l('j'), l('k'), l('l'),
			c(';', ':'), c("'", '"'),
			{ label: '⏎', action: 'enter', width: 1.75, wide: true }
		],
		[
			{ label: '⇧', mod: 'shift', width: 2, wide: true },
			l('z'), l('x'), l('c'), l('v'), l('b'), l('n'), l('m'),
			c(',', '<'), c('.', '>'), c('/', '?'),
			{ label: '⇧', mod: 'shift', width: 2, wide: true }
		],
		[
			{ label: 'Ctrl', mod: 'ctrl', width: 1.5, wide: true },
			{ label: 'Alt', mod: 'alt', width: 1.5, wide: true },
			{ label: 'Fn', action: '__fn', width: 1.5, wide: true },
			{ label: 'Space', action: 'space', width: 4 },
			{ label: '←', action: 'left', wide: true },
			{ label: '↑', action: 'up', wide: true },
			{ label: '↓', action: 'down', wide: true },
			{ label: '→', action: 'right', wide: true }
		]
	]
};

export const layouts: KeyboardLayout[] = [usQwerty];
