# Open issues

- `lumos` pairwise normalization of CFA drizzles (`combine/normalization/mod.rs`, `pairwise_norms`): a pair of frames that shares only a few pixels in a slot is fitted over those few as over thousands; no count of shared pixels separates a pair that can be fitted from one too thin to fit.
- `lumos` spilled combine under a memory cgroup (`combine/cache/core.rs`, `process_chunks`, over maps advised `Sequential`): when the cgroup's limit leaves the page cache less than a chunk's planes plus their readahead, the combine re-reads the same pages without end. 40 spilled frames of 2048 × 2048 (640 MiB), `memory_override` 128 MiB, in a user scope with `MemoryMax=320M` read 1.77 TB in ten minutes without finishing one pass; at `MemoryMax=480M`, 16 GB in 14 s.
