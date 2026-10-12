/**
 * Feature flags read from the environment at request time. A flag is on only for the exact value
 * `true`; anything else (unset, empty, `1`, `TRUE`, typos) is off, so a mistake never turns it on.
 */
export function chatEnabled(env: Record<string, string | undefined> = process.env): boolean {
	return env.ENZARB_UI_CHAT === 'true';
}
