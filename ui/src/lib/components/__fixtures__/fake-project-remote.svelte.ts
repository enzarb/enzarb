// Test double for the project queries used by the project page. Mirrors the shape of a SvelteKit
// client query (`current`, `error`, `loading`, `refresh()`), driven by `behavior`.
export const behavior: {
	project: unknown;
	projectError: unknown;
	settingsError: unknown;
	refreshError: unknown;
} = {
	project: undefined,
	projectError: undefined,
	settingsError: undefined,
	refreshError: undefined
};
export const counts = { refresh: 0 };

function fakeQuery(read: () => { value?: unknown; error?: unknown }) {
	let tick = $state(0);
	return {
		get current() {
			void tick;
			return read().value;
		},
		get error() {
			void tick;
			return read().error;
		},
		get loading() {
			return false;
		},
		refresh(): Promise<void> {
			counts.refresh++;
			if (behavior.refreshError) return Promise.reject(behavior.refreshError);
			tick++;
			return Promise.resolve();
		}
	};
}

export const getProject = () =>
	fakeQuery(() => ({ value: behavior.project, error: behavior.projectError }));
export const deleteProject = async () => {};
export const setMaxRuns = async () => {};
export const setSandboxResources = async () => {};
export const claudeCredential = () =>
	fakeQuery(() => ({
		value: behavior.settingsError ? undefined : { configured: false, updatedAt: null },
		error: behavior.settingsError
	}));
