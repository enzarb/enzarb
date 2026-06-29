import { sql, defaultSettings } from '$lib/db';

export type SettingKey = keyof typeof defaultSettings;

export interface PlatformSettings {
	/** Max workspace PVC size (GiB) allowed on the free tier. */
	freeMaxPvcGi: number;
	/** Days a soft-deleted project/org is recoverable before the operator purges it. */
	retentionDays: number;
	pricing: {
		vcpuHoursPerUnit: number;
		memGiBHoursPerUnit: number;
		blockStorageGiBMonthsPerUnit: number;
		registryGiBMonthsPerUnit: number;
		netIngressInternalPerGib: number;
		netEgressInternalPerGib: number;
		netIngressExternalPerGib: number;
		netEgressExternalPerGib: number;
		// Free-tier monthly allowances, one per billed metric.
		freeVCPUHours: number;
		freeMemGiBHours: number;
		freeBlockStorageGiBMonths: number;
		freeRegistryGiBMonths: number;
		freeNetIngressInternalGib: number;
		freeNetEgressInternalGib: number;
		freeNetIngressExternalGib: number;
		freeNetEgressExternalGib: number;
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
			vcpuHoursPerUnit: num('pricing_vcpu_hours_per_unit'),
			memGiBHoursPerUnit: num('pricing_mem_gib_hours_per_unit'),
			blockStorageGiBMonthsPerUnit: num('pricing_block_storage_gib_months_per_unit'),
			registryGiBMonthsPerUnit: num('pricing_registry_gib_months_per_unit'),
			netIngressInternalPerGib: num('pricing_net_ingress_internal_per_gib'),
			netEgressInternalPerGib: num('pricing_net_egress_internal_per_gib'),
			netIngressExternalPerGib: num('pricing_net_ingress_external_per_gib'),
			netEgressExternalPerGib: num('pricing_net_egress_external_per_gib'),
			freeVCPUHours: num('pricing_free_vcpu_hours'),
			freeMemGiBHours: num('pricing_free_mem_gib_hours'),
			freeBlockStorageGiBMonths: num('pricing_free_block_storage_gib_months'),
			freeRegistryGiBMonths: num('pricing_free_registry_gib_months'),
			freeNetIngressInternalGib: num('pricing_free_net_ingress_internal_gib'),
			freeNetEgressInternalGib: num('pricing_free_net_egress_internal_gib'),
			freeNetIngressExternalGib: num('pricing_free_net_ingress_external_gib'),
			freeNetEgressExternalGib: num('pricing_free_net_egress_external_gib')
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
