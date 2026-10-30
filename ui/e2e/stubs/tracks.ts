// Stand-in for src/lib/server/tracks.ts in the e2e web server: enzarb-tracks without the service.
// The "TOML" of both files is JSON here, and validation accepts everything, so the specs exercise
// the UI's own commit, refresh and render path against the real SvelteKit remote functions.
import type { ModesDoc } from '../../src/lib/modes-types.ts';
import type { WorkflowsFileWire } from '../../src/lib/workflows-types.ts';
import { TracksError, type TracksClient } from '../../src/lib/server/tracks.ts';

export * from '../../src/lib/server/tracks.ts';

const parse = <T>(text: string): T => {
	try {
		return JSON.parse(text) as T;
	} catch {
		throw new TracksError(422, 'invalid_modes_toml', 'not valid');
	}
};

const client: TracksClient = {
	modesCatalog: async () => ({
		builtinTools: [],
		hookEvents: [],
		permissionValues: ['allow', 'ask', 'deny'],
		onFailureValues: ['block', 'warn'],
		templates: []
	}),
	parseModes: async (text) => ({ doc: parse<ModesDoc>(text), issues: [] }),
	validateModes: async () => [],
	renderModes: async (doc) => ({ text: JSON.stringify(doc), issues: [], rewritten: [] }),
	checkWorkflows: async (text) => ({ diagnostics: [], file: parse<WorkflowsFileWire>(text) }),
	renderWorkflows: async (file) => ({ text: JSON.stringify(file), diagnostics: [] }),
	dryRun: async () => ({ results: [] }) as never
};

export const getTracks = (): TracksClient => client;
