# Runtime-cache findings: RAM residency and disk-cache loads

Investigation of `src/execution/{cache,resolve,executor,disk_store}` answering three
questions: where RAM held by cached node outputs could be freed sooner, where a disk-cache
load could be skipped (or shrunk), and whether one blob can be read from disk twice in a
single run. Findings only — no proposed changes. References are function names in the
current tree.

Vocabulary: "RAM-retained" = `CacheMode::caches_in_ram()` (`Ram`/`Both`); "disk-backed" =
`persists_to_disk()` (`Disk`/`Both`); "hydration" = `RuntimeCache::check_reuse` reading a
blob via `DiskStore::read` and installing it as a `ValueState::Resident` slot.

## A. RAM held longer than necessary

**A1. Hydration happens at resolve time, so hydrated RAM peaks before the run starts.**
`Resolver::resolve` awaits `check_reuse` for every frontier node in the reverse sweep, so
every disk-reused value in the run is loaded into RAM *before the first node executes*.
A value whose only consumer sits late in a long schedule is resident for the whole prefix
of the run instead of from just-before-first-read. This inflates peak RAM in exactly the
runs (long, disk-heavy) where it matters most.

## B. Disk loads that could be skipped or shrunk

**B1. `format::read` decodes every stored payload regardless of demand.**
The `demand` parameter of `DiskStore::read` only gates the *coverage* verdict (a demanded
port stored `Unbound` → miss). `read_header` collects **all** descriptors and the value
loop decodes every bound payload — including ports the run marked `Skip`. There is no
seek-past for undemanded payloads, so a hydration pays full-blob I/O and decode cost, and
the undemanded values then occupy RAM in the snapshot, no matter how narrow the actual
demand was.

**B2. Coverage expansion re-reads everything, discarding valid resident ports.**
In `check_reuse`, a resident current-digest value that lacks even one newly-demanded port
fails `is_resident_hit` and falls through to a full disk read whose result *replaces* the
whole snapshot. Ports already resident in RAM under the same digest are re-read from disk
(and, per B1, so is everything else in the blob). One newly-pinned port on a `Both`-mode
node is enough to trigger a full re-hydration of values the cache already holds.

**B3. Pin demand hydrates unconditionally, and unchanged pins re-hydrate every run.**
`seed_external_demand` seeds `Produce` for pinned roots and authored pinned ports with no
notion of whether the value already reached the host. For a `Disk`-mode node this means:
every run whose cone includes the pinned node hydrates the blob at resolve, clones the
value into a `PinnedOutputs` event, and immediately releases it — then does the same
again next run, digest unchanged, to deliver an identical preview. There is no
"delivered-under-this-digest" memory anywhere. Additionally, when `execute` runs with
`events: None`, `emit_pinned_values` returns without reading anything, so the hydration
that served only pin demand was wasted entirely (production worker always passes
`Some(&event_tx)`, so today this waste is confined to eventless callers).

**B4. The upstream cone of a late-improved reuse has already executed.**
A `Run` node whose digest folded an unreadable bound resource keeps its whole producer
cone alive ("uncacheable, must run"). At reach time the executor re-stamps and may find a
disk hit — but by then every upstream producer has already executed (they were ordered
first), solely to feed inputs that `abandon_input_reads` then discards unread. The disk
load itself is legitimate and single; the cone execution it obsoletes is the waste.

**B5. Every store is preceded by a header read, and a failed commit adds another.**
`DiskStore::store` calls `covers` (open + header/descriptor scan of the existing file)
before every write, including the common fresh-digest case where the scan is guaranteed to
answer "no" at the digest comparison. A failed commit runs `covers` a second time to
decide whether to warn. `store_resident_caches` (disk-store attach) does this probe for
every resident disk-backed node. These are header-only reads, not value loads, but they
are per-store synchronous file opens on the run's critical path (the executor awaits
`store_node` inline after each node).

## C. Can one blob be loaded twice in one run?

No — within a single `ExecutionEngine::execute`, each node's blob is value-read at most
once. The two read sites are mutually exclusive per node:

- **Resolve-time** (`resolve_run` → `check_reuse`): only reached when the stamped digest
  is `Some`; a hit installs the snapshot as `Resident`, so any later probe is a RAM hit.
- **Reach-time improvement** (executor): only entered when the *resolve-time* digest was
  `None` — and in that case the resolve-time `check_reuse` returned at the digest guard
  before touching disk. After the reach-time re-stamp, disk is read once.

A resolver `Reuse` verdict is never re-derived in the executor (documented invariant), so
no second probe exists for the same node. A failed decode deletes the blob and reports a
miss with no retry. The remaining same-run disk *accesses* to a blob are header-only:
`covers` before each store and after a failed commit (B5) — they never load values.

Across runs, by design of `Disk` mode, the same blob is re-read once per run that demands
it (the RAM copy is dropped by `release_dead_outputs`); combined with B1/B3 this is the
dominant source of repeated disk I/O for unchanged graphs.

## D. Fix sequencing

Batches, not one-by-one: several findings are a single policy expressed as multiple
symptoms, and the natural verification granularity (one module's test suite + the RAM
stats a run reports) matches the cluster, not the individual finding. Each batch below is
small, independently landable, and touches a disjoint area, so they can ship in any order
— the listed order is payoff-per-risk.

**Batch 1 — disk read/write path: B1, B2, B5** (`disk_store/format`, `check_reuse`).
B2's proper shape depends on B1: only once the reader can skip or select payloads can
coverage expansion read just the missing ports and merge instead of replace. B5 is the
same module's write side. Verified by the existing format/disk-store tests plus new
partial-read cases.

**Batch 2 — pin delivery memo: B3** (report/worker). Independent mechanism
(delivered-under-digest tracking) with host-protocol implications; nothing else depends
on it and it depends on nothing above.

**Deferred, revisit after batch 1: A1, B4.** A1 (reach-time hydration) is the one
architectural change — it reshapes the resolve/execute contract, and batch 1 changes its
economics enough that it should be re-measured afterwards rather than built now. B4 is
inherent to late-bound resources; fixing it means deferring cone execution
until after reach-time re-stamping, i.e. a scheduler redesign — out of proportion to the
waste unless profiles say otherwise.
