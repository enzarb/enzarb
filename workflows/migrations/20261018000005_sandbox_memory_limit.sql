-- The sandbox's cgroup `memory.max` (stats frame `memory_limit_bytes`), for the memory
-- percentage platform fact. Additive and nullable: NULL means unlimited or not reported.
ALTER TABLE engine.sandbox_stats ADD COLUMN memory_limit_bytes bigint;
