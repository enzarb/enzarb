import type { Sql } from './db.ts';

export const READY_TIMEOUT_MS = 2000;

/** True when the database answers `SELECT 1` within the timeout. Never throws. */
export async function isDatabaseReady(sql: Sql, timeoutMs = READY_TIMEOUT_MS): Promise<boolean> {
	let timer: ReturnType<typeof setTimeout> | undefined;
	const timeout = new Promise<never>((_, reject) => {
		timer = setTimeout(() => reject(new Error('database readiness timeout')), timeoutMs);
	});
	try {
		await Promise.race([sql`SELECT 1`, timeout]);
		return true;
	} catch {
		return false;
	} finally {
		clearTimeout(timer);
	}
}
