// Shared, per-workspace reachability tracker. All tabs (terminal, files,
// agents) gate their connections on ensureHealthy() so that a restarting
// workspace pod pauses every reconnect loop instead of burning retry
// attempts against a gateway 5xx, and the project layout can show a single
// "workspace restarting" overlay driven by the same state.

export type WorkspaceHealthState = 'unknown' | 'healthy' | 'unhealthy';

const PROBE_TIMEOUT_MS = 3_000;
const POLL_INTERVAL_MS = 2_500;

export class WorkspaceHealth {
	state: WorkspaceHealthState = $state('unknown');
	private waiters: (() => void)[] = [];
	private polling = false;

	constructor(private agentBase: string) {}

	private async probe(): Promise<boolean> {
		try {
			const res = await fetch(`${this.agentBase}/healthz`, {
				signal: AbortSignal.timeout(PROBE_TIMEOUT_MS)
			});
			return res.ok;
		} catch {
			return false;
		}
	}

	/** Resolves once the workspace agent is reachable. Instant when already
	 *  known-healthy; otherwise probes, and if the pod is down waits for the
	 *  recovery poll loop instead of letting the caller attempt a connection. */
	async ensureHealthy(): Promise<void> {
		if (this.state === 'healthy') return;
		if (await this.probe()) {
			this.state = 'healthy';
			return;
		}
		this.markUnhealthy();
		await new Promise<void>((resolve) => this.waiters.push(resolve));
	}

	/** Call when a connection failed in a way that suggests the pod is gone
	 *  (WS drop, fetch network error). Downgrades to 'unknown' so the next
	 *  ensureHealthy() re-probes instead of trusting the cached 'healthy'. */
	suspect(): void {
		if (this.state === 'healthy') this.state = 'unknown';
	}

	/** Call when the workspace is known to be going down (e.g. the user just
	 *  requested a restart). Starts polling /healthz until it recovers, then
	 *  releases everyone waiting in ensureHealthy(). */
	markUnhealthy(): void {
		this.state = 'unhealthy';
		if (this.polling) return;
		this.polling = true;
		const tick = async () => {
			if (await this.probe()) {
				this.polling = false;
				this.state = 'healthy';
				const waiters = this.waiters;
				this.waiters = [];
				for (const resolve of waiters) resolve();
			} else {
				setTimeout(tick, POLL_INTERVAL_MS);
			}
		};
		setTimeout(tick, POLL_INTERVAL_MS);
	}
}

const trackers = new Map<string, WorkspaceHealth>();

export function workspaceHealth(agentBase: string): WorkspaceHealth {
	let tracker = trackers.get(agentBase);
	if (!tracker) {
		tracker = new WorkspaceHealth(agentBase);
		trackers.set(agentBase, tracker);
	}
	return tracker;
}
