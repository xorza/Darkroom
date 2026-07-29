# The reuse path — investigation

Follow-up to the "reuse path is written three times" group in `cache-review.md`.
The textual duplication is real but shallow; the thing underneath it is not what
the review said it was.

---

## What the call graph actually shows

`hydrate_reuse` has **two callers with opposite preconditions**, and only the
caller knows which one it is:

| caller | reached via | was there a probe? | producer cone | failure means |
| --- | --- | --- | --- | --- |
| `executor::serve_reuse` | `NodeState::Reuse` | **yes**, in `resolve` | **cut** | unrecoverable → `RunError::CacheLoadFailed` |
| `executor::needs_invoke` → `restamp_and_hydrate` | `NodeState::Run`, digest was `None` | **no** | **alive** | fine → returns `false`, node runs |

So `hydrate_reuse` is not "the second half of `probe_reuse`". It is a standalone
*serve-if-you-can* used in one recoverable and one unrecoverable context.

`CacheLoadFailed` is not the cost of duplication. It is the cost of
`resolve/mod.rs:126` **cutting the producer cone on the strength of a verdict
that a different function, at a different time, has to make good on** — with
nothing carried between them. `probe_reuse` returns a bare `bool`; the evidence
behind it (which blob, which digest, which descriptors) is discarded, and
`hydrate_reuse` re-derives all of it from scratch.

## What is genuinely duplicated

**1. The `format` layer is already shared** — not a finding.
`covers_demand` is `read_header(..).is_some()`; `read` is `read_header(..)` plus
the body decode. One header scan, two entry points. Correct as-is.

**2. `DiskStore` repeats the open-and-measure preamble 3×** — mechanical.
`covers:111`, `covers_demand:129`, `read:141` each open the file and take
`metadata().len()`. The three failure policies differ only in *logging*, and the
difference is justified: a miss is expected for the two `covers*`, while `read`
runs after something already promised the blob is there, so an unexpected error
is worth a warning. The outcome (treat as miss) is identical in all three.

**3. `RuntimeCache` repeats `is_resident_hit` → `blob_target` 2×** — mechanical.
`blob_target` re-derives `(e_node_id, e_node, digest)` from `(program, node_idx)`
on every call, and is called up to 3× per node per run (probe, hydrate, store).

## What "properly addressing" it means

Three levels, increasing in what they buy and what they cost.

### Level 1 — one blob opener (safe, mechanical)

One `DiskStore` helper returning `io::Result<Option<(File, u64)>>`, where
`Ok(None)` is NotFound. `covers`/`covers_demand` do `.ok().flatten()`; `read`
matches and warns on `Err`. Removes 3 copies, preserves all 3 policies exactly.
No behaviour change.

### Level 2 — one reuse preamble (safe, mechanical)

A private `reuse_source(node_idx, demand) -> Option<ReuseSource>` with
`ReuseSource::{Resident, Blob(BlobTarget)}`. `probe_reuse` and `hydrate_reuse`
become one line each over the same match. No behaviour change; does **not**
narrow the probe/hydrate window.

### Level 3 — carry the probe's evidence (the real fix, needs a decision)

The probe already opened the blob and validated its header. Keeping the
`BlobTarget` it approved — on the slot, or in a per-run column beside
`NodeState::Reuse` — would mean:

- `hydrate_reuse` serves *the target the probe approved*, not one it re-derives;
- the `blob_target` recomputation disappears from the hydrate path (and the
  `store_node` path could reuse it too);
- `NodeState::Reuse` carries the evidence that justified cutting the cone,
  instead of being a bare marker whose justification was thrown away.

**It does not eliminate `CacheLoadFailed`,** and no design can: the header is
validated at probe time and the body is read later, with no lock in between, so
a body that rots in the gap is always possible. What it removes is every *other*
way the two can disagree — a re-derived target, a digest restamped between the
two points, a demand slice read twice.

### Rejected: collapse the two phases

Making the probe decode (so there is one path) is the obvious "fix" and is
wrong. `runtime/mod.rs:574-581` states the reason: decoding at probe time would
pull every frontier blob into RAM before the first lambda runs, instead of
interleaving decodes with execution. The two-phase split is load-bearing. So is
the header-only probe — it is what lets `resolve` prune a cone without paying
for a decode.

## Recommendation

Levels 1 and 2 are pure deduplication and can land together with no behavioural
risk. Level 3 is the one worth arguing about: it changes what a `Reuse` state
carries, touches the resolver, and its payoff is narrowing a race rather than
closing it. Worth doing only if `CacheLoadFailed` has actually been observed —
otherwise it is complexity spent on a window that is already narrow.

The review item should be reworded either way: the problem is not that the reuse
path is written three times, it is that **one function serves two contracts and
the caller's context is what distinguishes them**.
