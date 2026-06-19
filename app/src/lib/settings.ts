import { sql, defaultSettings } from '$lib/db';

export type SettingKey = keyof typeof defaultSettings;

export interface PlatformSettings {
	/** Max workspace PVC size (GiB) allowed on the free tier. */
	freeMaxPvcGi: number;
	/** Days a soft-deleted project/org is recoverable before the operator purges it. */
	retentionDays: number;
	pricing: {
		cpuSecondsPerUnit: number;
		memGiBSecondsPerUnit: number;
		netIngressPerByte: number;
		netEgressPerByte: number;
		storageGiBSecondsPerUnit: number;
		freeCPUSeconds: number;
		freeMemGiBSeconds: number;
	};
}

async function rawSettings(): Promise<Record<string, string>> {
	const rows = await sql`SELECT key, value FROM app_settings`;
	const merged: Record<string, string> = { ...defaultSettings };
	for (const row of rows) merged[row.key] = row.value;
	return merged;
}

/** Reads admin-editable platform settings, falling back to seeded defaults. */
export async function getSettings(): Promise<PlatformSettings> {
	const s = await rawSettings();
	const num = (k: SettingKey) => Number(s[k]);
	return {
		freeMaxPvcGi: num('free_max_pvc_gi'),
		retentionDays: num('retention_days'),
		pricing: {
			cpuSecondsPerUnit: num('pricing_cpu_seconds_per_unit'),
			memGiBSecondsPerUnit: num('pricing_mem_gib_seconds_per_unit'),
			netIngressPerByte: num('pricing_net_ingress_per_byte'),
			netEgressPerByte: num('pricing_net_egress_per_byte'),
			storageGiBSecondsPerUnit: num('pricing_storage_gib_seconds_per_unit'),
			freeCPUSeconds: num('pricing_free_cpu_seconds'),
			freeMemGiBSeconds: num('pricing_free_mem_gib_seconds')
		}
	};
}

/** Persists one or more settings keys. Caller is responsible for authorization. */
export async function updateSettings(values: Partial<Record<SettingKey, string>>): Promise<void> {
	const entries = Object.entries(values).filter(([k]) => k in defaultSettings);
	for (const [key, value] of entries) {
		await sql`
			INSERT INTO app_settings (key, value, updated_at)
			VALUES (${key}, ${value as string}, now())
			ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()
		`;
	}
}
