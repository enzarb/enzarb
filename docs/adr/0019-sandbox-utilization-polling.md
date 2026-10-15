# 0019. Polling sandbox utilization from the sandbox-init API

## Context
Nothing collects sandbox CPU, memory, storage or disk I/O today (gap 11, epic #135). sandbox-init must not push metrics: something polls it. This ADR fixes the source, the wire shape, the poller, the table and the window query, so the follow-up issues `sandbox-stats` (sandbox-init and `proto/`), `sandbox-store` (migration and `workflows_api`), `sandbox-poller` (`workflows/`) and `sandbox-ui` can be built without more design.

## Decision

### 1. Stats source (sandbox-init)
- sandbox-init reads the cgroup v2 files of **its own cgroup** (path from `/proc/self/cgroup`, unified mount at `/sys/fs/cgroup`): `cpu.stat` (`usage_usec`, and `throttled_usec` if present), `memory.current`, `memory.max` (`max` means no limit), `io.stat` (`rbytes` and `wbytes` summed over all devices), and `statfs` of `/workspace` for used and total bytes.
- dockerd is supervised inside the same container (ADR-0016), so its containers are descendants in the same cgroup subtree. The container's own cgroup therefore includes them; no per-container breakdown is made.
- Values are **raw**: cumulative counters (`cpu_usec`, `cpu_throttled_usec`, `io_read_bytes`, `io_write_bytes`) or gauges (`mem_bytes`, `mem_limit_bytes`, `storage_used_bytes`, `storage_total_bytes`). Each reply carries `ts` (unix ms from the sandbox's clock) and `boot_id` (random, new per sandbox-init start). Rates are derived on read. sandbox-init keeps no history.
- Under gVisor (ADR-0012) cgroup files may be missing or partial. Every field is optional: absent means "not reported", never 0 (as for `Usage`), and the poller stores NULL. This is **unverified**: `sandbox-stats` must check the real behaviour under gVisor, the only runtime (ADR-0012) and record it in `docs/contracts.md` section 3.

### 2. API shape (`proto/`)
- A new **additive CTRL request/reply on channel 0**, the same pattern as `credential_status`/`credential_state`:
  - `stats_request` (client to server, no fields);
  - `stats` (server to client): `{ts, boot_id, cpu_usec?, cpu_throttled_usec?, mem_bytes?, mem_limit_bytes?, io_read_bytes?, io_write_bytes?, storage_used_bytes?, storage_total_bytes?}`. All numbers are non-negative integers.
- Gated by a new `welcome.features` entry `stats`. A client sends `stats_request` only after the `welcome` listed it: an old sandbox answers `unknown_message`, and an old proxy closes the socket on a frame it cannot decode (the "old proxy" hazard in the protocol spec). The hazard applies to browser clients, which go through sandbox-proxy. The poller is an executor-side client that connects to the sandbox directly, as run dispatch does, so no proxy sits in its path; the feature gate still protects it from old sandboxes.
- Protocol version stays 1 (ADR-0003). No new channel kind: a reply is one small JSON object and needs no flow control.
- **Auth:** the existing bearer token on the websocket upgrade. The poller resolves the target and token exactly as run dispatch does (`SandboxResolver`) and says `client: "executor"` in `hello`. No extra credential travels in the protocol. sandbox-proxy does not relay `stats` to browsers; the UI reads Postgres.

### 3. Poller (`workflows/`)
- A background loop in the executor crate, next to the ticker. Only one replica polls at a time: it holds a Postgres session advisory lock, and the others retry the lock each interval.
- **Discovery:** the `SandboxResolver` gains `list_projects` (additive), which lists `Project` CRs. A Project whose sandbox is not ready (`NotReady`: not provisioned, starting or stopped) is skipped silently and nothing is written.
- **Interval:** 30 s by default, `ENZARB_STATS_INTERVAL`, accepted range 15 to 300 s (a value outside the range is clamped and logged). Each poll has a 5 s timeout; concurrency is bounded (about 8 projects at once) and start times are jittered.
- **Failures:** unreachable, timeout, `unknown_message`, `unimplemented` or a missing `stats` feature all mean *skip the sample*. No zero rows and no gap-filling. Consecutive failures back off per project (doubling up to 5 min) and reset on success. They are logged at debug with a per-round count of ok/skipped/failed, since there is no metrics system yet.
- **Work item attribution:** the sandbox is per Project, and worktrees share one cgroup, so attribution is approximate. At sample time the poller looks up work items with a `running` agent job on that Project. One sample row is stored once, with the lowest such `work_item_id`, or NULL when none runs.

### 4. Storage
Table `workflows.sandbox_stats` (migration in `workflows/migrations`):

| column | type | note |
|---|---|---|
| `project` | text | Project name |
| `sandbox` | text | pod name; changes on restart |
| `work_item_id` | bigint null | approximate, see above |
| `at` | timestamptz | the sandbox's `ts`, not the poller's clock |
| `boot_id` | text | |
| `cpu_usec`, `cpu_throttled_usec`, `io_read_bytes`, `io_write_bytes` | bigint null | counters |
| `mem_bytes`, `mem_limit_bytes`, `storage_used_bytes`, `storage_total_bytes` | bigint null | gauges |

- Primary key `(project, sandbox, at)` (so a retried insert is `ON CONFLICT DO NOTHING`); index `(project, at)`.
- **Retention:** 30 days by default (`ENZARB_STATS_RETENTION`). The poller deletes older rows in batches (for example 10 000 per statement) once an hour.
- **Growth bound:** at 30 s that is about 86 000 rows per sandbox over 30 days, about 150 bytes each, so roughly 13 MB. Besides the 15 s interval floor, the hourly prune caps each `(project, sandbox)` at 150 000 rows by deleting the oldest. At the 15 s floor, 30 days is about 172 800 rows, so the cap shortens retention to about 26 days at the fastest interval; that is accepted, and the cap is a safety bound rather than the retention rule. Projects that no longer exist leave rows that age out by retention.
- **Contract (additive, `workflows_api`):** read-only view `sandbox_stats_v` (same columns, ordered by `at`) and a function `workflows_api.sandbox_stats_window(p_project text, p_from timestamptz, p_to timestamptz, p_buckets int)`. `enzarb_ui` gets `SELECT` on the view and `EXECUTE` on the function and nothing on the table; the UI checks project access before calling, as for the other views.

### 5. Window query (downsampling)
- Bucket width is `(p_to - p_from) / p_buckets`. `p_buckets` is clamped to 1..300 and the window to the retention period.
- **Gauges** (memory, storage): per bucket `avg` and `max`. Limits and totals: the last value in the bucket.
- **Counters** (cpu, io): for each pair of consecutive samples of the same `(sandbox, boot_id)` the rate is `delta / Δt`: CPU in cores (`Δusec / (Δt in µs)`), I/O in bytes per second. The interval belongs to the bucket of its later sample. A bucket returns `avg` and `max` of its rates.
- A **negative delta** (counter reset), a change of `boot_id` or sandbox, or a NULL on either side drops that interval; it is never shown as a negative value or a spike. An interval longer than 3x the configured poll interval is a gap: it is dropped, not averaged across.
- A bucket without usable samples returns **no row**, and the UI draws a gap.
- `cpu_throttled_avg` is in cores, like `cpu_cores_*`: the delta of `cpu_throttled_usec` divided by the elapsed microseconds, so 0.5 means half a core's worth of time spent throttled.
- Returned columns (append-only): `bucket_start`, `samples`, `cpu_cores_avg`, `cpu_cores_max`, `cpu_throttled_avg`, `mem_bytes_avg`, `mem_bytes_max`, `mem_limit_bytes`, `io_read_bps_avg`, `io_read_bps_max`, `io_write_bps_avg`, `io_write_bps_max`, `storage_used_bytes_avg`, `storage_used_bytes_max`, `storage_total_bytes`.

### 6. Acceptance tests of the follow-ups
- `sandbox-stats`: cgroup parsing fixtures (normal, `memory.max` = `max`, missing files, several io devices); a proto round-trip vector for both frames in Rust and TS; `welcome.features` contains `stats`; the gVisor behaviour is documented.
- `sandbox-store`: the migration applies; privileges of `enzarb_ui`; window-query tests for a counter reset, a `boot_id` change, a gap longer than 3x the interval, empty buckets, samples exactly on bucket edges, and `p_buckets` clamping.
- `sandbox-poller`: backoff growth and reset; skipping of `NotReady` and of sandboxes without the feature; a retried insert is idempotent; the retention prune and the row cap; only one replica polls.
- `sandbox-ui`: charts leave gaps for missing buckets; NULL fields are shown as "not reported".

Rejected alternatives:
- *sandbox-init pushes samples.* Against maintainer feedback; it would need credentials and a store endpoint in the sandbox.
- *An HTTP `/metrics` endpoint on sandbox-init.* A second port and a second auth path.
- *A Prometheus or OpenTelemetry stack.* Out of MVP scope (`mvp.md`).
- *Pre-computed rates in sandbox-init.* It would have to keep history, and a lost sample would corrupt the rate.
- *A separate `(sample, work_item)` link table.* More weight than approximate attribution is worth.
- *A new schema for stats.* The poller lives in `workflows`, which already owns `workflows_api`.

## Consequences
- Additive everywhere: two CTRL frames and a feature, a table, a view and a function. Old sandboxes keep working and are simply not charted; old consumers ignore the additions.
- Stats cost one request per sandbox per interval and a small table. There is no history in the sandbox, so samples are lost while the poller is down.
- Work item attribution is approximate and the sandbox's cgroup includes everything in it (dockerd, builds, terminals).
- Not done: alerting, per-process or per-container breakdown, network metrics, Claude usage limits, external metrics export, rollups beyond 30 days.
