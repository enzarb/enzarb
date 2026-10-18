-- The sandbox's `memory.stat` `inactive_file` (stats frame `memory_inactive_file_bytes`):
-- reclaimable page cache inside `memory.current`, subtracted for the working-set percentage.
-- Additive and nullable: NULL means not reported (an older sandbox).
ALTER TABLE engine.sandbox_stats ADD COLUMN memory_inactive_file_bytes bigint;
