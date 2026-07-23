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

**A1. Impure RAM-retained nodes hold outputs that can never be reused.**
`node_digest` returns `None` for any non-`Pure` node, so an impure `Ram`/`Both` node's slot
is always `Resident { produced_under: None }` with `current_digest: None`. `check_reuse`
bails on a `None` digest, `is_resident_current` is always false, and the value is never
stored to disk (`blob_target` needs a digest). Yet every release path skips it:
`ExecutionFrame::finish_read` and the post-invoke drain release both early-return on
`caches_in_ram()`, and `release_non_ram_outputs` skips it too. Result: the full output
snapshot of every impure RAM-retained node stays resident between runs as pure dead
weight — it can serve within-run consumer reads, but after its last planned read of the
producing run it has no possible future use until the node itself re-runs and overwrites
the buffer in place.

**A2. Superseded-digest values on RAM-retained nodes are kept on a flip-back-only bet.**
A `Ram`/`Both` node whose digest changed but which is not invoked this run (disposition
`Cut`, or outside the planned cone) keeps its old resident value indefinitely. The value
can only ever hit again if the node's digest returns to exactly `produced_under` (input
flipped back) before the node next runs. That retention is deliberate (`check_reuse` doc:
"however the value came to be resident"), but it is unconditional: RAM is held at full
size for a speculative exact-revert hit, with no signal distinguishing it from a
current-valid resident value except `Cut { cached: false }` in the stats.

**A3. Reused-with-zero-readers values are held to end-of-run; freshly-ran ones are not.**
After a successful invoke, the executor immediately releases a non-RAM node whose reads
are already drained ("a sink, or every output already `Skip`" — the post-`store_node`
check). The `Disposition::Reuse` branch has no equivalent: a non-RAM node hydrated from
disk whose demand came only from a pin / seeded preview root (readers all zero) emits its
pinned values and then sits resident until `release_non_ram_outputs` after the whole run.
A potentially large preview value occupies RAM for the entire run in the reuse case but
would have been dropped instantly in the ran case.

**A4. Planned reads are never retired when the consumer is skipped.**
`mark_skipped` (errored upstream, missing lambda) clears the *skipped node's* output but
does not walk its bound inputs the way `cancel_input_reads` does for the late-reuse
improvement. The skipped consumer's planned reads therefore stay in `RemainingOutputReads`
forever, so a non-RAM producer read only by skipped consumers never drains mid-run and is
held until the end-of-run sweep — even though the moment the consumer was skipped, the
value provably had no remaining reader. The cancelled-run tail behaves the same way (loop
`break` leaves all remaining reads unconsumed).

**A5. Outputs produced despite `Skip` demand are retained.**
A lambda may bind outputs whose demand was `Skip` (cheap byproducts). `invoke_slot` keeps
whatever was written; nothing clears skip-demanded ports after a successful run. On
RAM-retained nodes those values stay resident across runs even though no reader or pin
demanded them (they do widen `covers_demand` for future runs; the per-node RAM stats count
them but nothing attributes them to undemanded ports). Hydration has the same effect from the other direction — see B2.

**A6. A cache-mode downgrade frees RAM only at the end of the *next* run.**
`RuntimeCache::reconcile` on install preserves slots by id and never consults the new
program's `CacheMode`. A node edited from `Ram`/`Both` to `None`/`Disk` keeps its resident
value through the install; the value is swept only by `release_non_ram_outputs` at the end
of the next `execute`. If no run follows, it sits indefinitely. (Until that sweep the
stale-mode value can still serve a RAM hit, since `is_resident_hit` deliberately ignores
the mode.)

**A7. Hydration happens at resolve time, so hydrated RAM peaks before the run starts.**
`Resolver::resolve` awaits `check_reuse` for every frontier node in the reverse sweep, so
every disk-reused value in the run is loaded into RAM *before the first node executes*.
A value whose only consumer sits late in a long schedule is resident for the whole prefix
of the run instead of from just-before-first-read. This inflates peak RAM in exactly the
runs (long, disk-heavy) where it matters most.

## B. Disk loads that could be skipped or shrunk

**B1. Demand from statically unexecutable consumers still drives hydration (and runs).**
The resolver never checks `e_node.lambda.is_none()` — that is discovered per node in the
executor, which then reports `MissingLambda` and skips. A consumer with no implementation
is classified `Run` by the sweep, so it seeds `Produce` demand and reader counts on its
producers; those producers then hydrate from disk (or execute) to feed a node that was
never able to run. The information is static — it lives on the installed program — so the
wasted loads/runs recur every run until the library is fixed.

**B2. `format::read` decodes every stored payload regardless of demand.**
The `demand` parameter of `DiskStore::read` only gates the *coverage* verdict (a demanded
port stored `Unbound` → miss). `read_header` collects **all** descriptors and the value
loop decodes every bound payload — including ports the run marked `Skip`. There is no
seek-past for undemanded payloads, so a hydration pays full-blob I/O and decode cost, and
the undemanded values then occupy RAM in the snapshot, no matter how narrow the actual
demand was.

**B3. Coverage expansion re-reads everything, discarding valid resident ports.**
In `check_reuse`, a resident current-digest value that lacks even one newly-demanded port
fails `is_resident_hit` and falls through to a full disk read whose result *replaces* the
whole snapshot. Ports already resident in RAM under the same digest are re-read from disk
(and, per B2, so is everything else in the blob). One newly-pinned port on a `Both`-mode
node is enough to trigger a full re-hydration of values the cache already holds.

**B4. Pin demand hydrates unconditionally, and unchanged pins re-hydrate every run.**
`seed_external_demand` seeds `Produce` for pinned roots and authored pinned ports with no
notion of whether the value already reached the host. For a `Disk`-mode node this means:
every run whose cone includes the pinned node hydrates the blob at resolve, clones the
value into a `PinnedOutputs` event, and releases it at end-of-run — then does the same
again next run, digest unchanged, to deliver an identical preview. There is no
"delivered-under-this-digest" memory anywhere. Additionally, when `execute` runs with
`events: None`, `emit_pinned_values` returns without reading anything, so the hydration
that served only pin demand was wasted entirely (production worker always passes
`Some(&event_tx)`, so today this waste is confined to eventless callers).

**B5. The upstream cone of a late-improved reuse has already executed.**
A `Run` node whose digest folded an unreadable bound resource keeps its whole producer
cone alive ("uncacheable, must run"). At reach time the executor re-stamps and may find a
disk hit — but by then every upstream producer has already executed (they were ordered
first), solely to feed inputs that `cancel_input_reads` then discards unread. The disk
load itself is legitimate and single; the cone execution it obsoletes is the waste.

**B6. Every store is preceded by a header read, and a failed commit adds another.**
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
`covers` before each store and after a failed commit (B6) — they never load values.

Across runs, by design of `Disk` mode, the same blob is re-read once per run that demands
it (the RAM copy is dropped by `release_non_ram_outputs`); combined with B2/B4 this is the
dominant source of repeated disk I/O for unchanged graphs.

## D. Fix sequencing

Batches, not one-by-one: several findings are a single policy expressed as multiple
symptoms, and the natural verification granularity (one module's test suite + the RAM
stats a run reports) matches the cluster, not the individual finding. Each batch below is
small, independently landable, and touches a disjoint area, so they can ship in any order
— the listed order is payoff-per-risk.

**Batch 1 — end-of-run release policy: A1, A2, A6** (`cache/mod.rs`,
`release_non_ram_outputs` and friends). All three are the same question: which resident
values does the end-of-run sweep consider dead? (never-hittable impure values,
superseded-digest values, mode-downgraded slots). One policy decision, one sweep, one test
fixture family. A2 needs an explicit call on whether flip-back retention stays — the only
finding here that is a deliberate tradeoff rather than an oversight.

**Batch 2 — mid-run read retirement: A3, A4, B1** (`executor/mod.rs` + one gate in
`resolve/mod.rs`). Complementary halves of one invariant — "a planned read is retired
exactly when it can no longer happen": the resolver stops seeding demand from
statically-unexecutable consumers (B1), the executor retires reads at skip time (A4) and
releases zero-reader reused slots symmetrically with the ran path (A3). Shares the
`RemainingOutputReads` machinery and the executor test suite. Note B1 shrinks but does not
remove A4 (`SkippedUpstream` is dynamic).

**Batch 3 — disk read/write path: B2, B3, B6** (`disk_store/format`, `check_reuse`).
B3's proper shape depends on B2: only once the reader can skip or select payloads can
coverage expansion read just the missing ports and merge instead of replace. B6 is the
same module's write side. Verified by the existing format/disk-store tests plus new
partial-read cases.

**Batch 4 — pin delivery memo: B4** (report/worker). Independent mechanism
(delivered-under-digest tracking) with host-protocol implications; nothing else depends
on it and it depends on nothing above.

**Deferred, revisit after batches 1–3: A5, A7, B5.** A5 is a policy question (byproduct
outputs widen future disk coverage — arguably a feature). A7 (reach-time hydration) is
the one architectural change — it reshapes the resolve/execute contract, and batch 3
changes its economics enough that it should be re-measured afterwards rather than built
now. B5 is inherent to late-bound resources; fixing it means deferring cone execution
until after reach-time re-stamping, i.e. a scheduler redesign — out of proportion to the
waste unless profiles say otherwise.
