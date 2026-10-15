const BYTE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];

/** A missing value is never shown as 0. */
export const NOT_REPORTED = 'not reported';

const valid = (n: number | null | undefined): n is number =>
	n !== null && n !== undefined && Number.isFinite(n) && n >= 0;

/** Bytes with a binary unit, e.g. `1.5 GiB`; "not reported" for null or non-finite. */
export function formatBytes(n: number | null | undefined): string {
	if (!valid(n)) return NOT_REPORTED;
	let v = n;
	let i = 0;
	while (v >= 1024 && i < BYTE_UNITS.length - 1) {
		v /= 1024;
		i++;
	}
	const text = i === 0 || v >= 100 ? v.toFixed(0) : v.toFixed(1).replace(/\.0$/, '');
	return `${text} ${BYTE_UNITS[i]}`;
}

export function formatBytesPerSec(n: number | null | undefined): string {
	return valid(n) ? `${formatBytes(n)}/s` : NOT_REPORTED;
}

/** CPU in cores, e.g. `0.25 cores`. */
export function formatCores(n: number | null | undefined): string {
	if (!valid(n)) return NOT_REPORTED;
	const text = String(Number(n >= 10 ? n.toFixed(1) : n.toFixed(2)));
	return `${text} ${text === '1' ? 'core' : 'cores'}`;
}
