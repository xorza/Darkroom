# Simplifying the resource-stamp system

An investigation into `scenarium`'s ResourceStamp / resource-hashing machinery:
whether digest computation can simply be delayed until all input values are
known, and where the actual accidental complexity lives.

## Verdict

The "delay digest until values are known" idea already exists in the system —
it is the late-restamp path in the executor, applied only where unavoidable.
Making it the *universal* rule would delete the taint/restamp machinery but
would also kill the resolver's backward cut, which is the property that makes
reopening a document cheap. The actual accidental complexity is elsewhere:
**the custom `ResourceStamper` half of the system has zero production users**
(no `with_stamper` anywhere in `lens`, `lumos`, `darkroom`, or `imaginarium` —
only scenarium's own tests), and it is precisely the half that carries all the
ugly parts. Deleting it shrinks the system by roughly half with no loss.

## Why "delay everything until values are known" regresses

Digests are deliberately structural: a node's digest folds its producers'
*digests*, not their output values (`node_digest` in
`scenarium/src/execution/digest/mod.rs`). That is what lets the resolver stamp
the entire graph before anything runs, and it powers the reverse sweep in
`scenarium/src/execution/resolve/mod.rs`: a consumer that is a cache hit stops
the walk, so its upstream cone gets `Disposition::Cut` and is never invoked or
even hydrated. Concretely: reopen a document whose final node has a disk blob →
only that node hydrates; the whole decode chain upstream is pruned.

If digests were computed at reach time instead (executor walks producer-first,
values in hand), every reuse check would be exact and the `Option`-taint
threading would vanish — but the executor meets producers *before* their
consumers, so it cannot know "this producer feeds only a future cache hit."
The cut dies, and with it exact pre-run output demand and reader counts, which
come out of the same reverse sweep. Every intermediate would hydrate or run on
reopen. For image pipelines that is a real cost, not a refactor-neutral change.

The one case where a digest genuinely needs a runtime value — a Bind-delivered
resource reference, e.g. an upstream-computed path string feeding an `FsPath`
input — is exactly the case the system already defers: taint to `None`, keep
the cone alive, restamp at reach time
(`scenarium/src/execution/executor/mod.rs`, the `Disposition::Run` +
`current_digest.is_none()` branch). The delay-when-needed is the correct
minimal shape; the architecture is not the problem.

## Where the real complexity lives

1. **The custom-stamper subsystem is dead weight.** `InputStamper::Custom`,
   `CustomResourceKey`/`CustomValueKey`/`CustomRequest`, `hash_custom`, the
   library's `stamper` field and its enum-validation, the `ResourceStamper`
   trait's cancel-token contract — all of it serves no caller outside
   scenarium's tests.

2. **It is also where the subtle hazards are.** Memoization is keyed by
   `Arc::as_ptr` pointer identity, and the fallback
   `CustomValueKey::Source(address)` arm has a latent staleness bug:
   `collect_node` at prepare-run time reads `output_values()` with no currency
   check (`scenarium/src/execution/resource/mod.rs`), so a stamp of a *stale*
   value can be recorded under the port-address key; after the producer
   re-runs, `prepare_node` sees the key already present and skips restamping,
   and the digest folds the old value's stamp. Probably unreachable today
   because Custom-typed wires deliver Arc-backed values (pointer-keyed arm),
   but it is the kind of trap this design keeps generating.

3. **Triplicated structs and a duplicated traversal.**
   `ResourceStampRequests` → `PreparedResourceStamps` → `RunResourceStamps`
   are three near-identical shapes, and `collect_node` re-implements the
   digest fold's binding walk — two places that must agree on which bindings
   are resource-relevant. That is the "over-intrusive" feel: resource
   awareness is smeared across `resource`, `digest`, `resolve`, `executor`,
   `program`, `flatten`, and `library`.

4. **Dual identity representations.** FsPath identities are stored
   structurally (`FsPathId` with per-entry folding) while custom stamps are
   opaque bytes; two maps, two hash paths in the digest fold.

## Recommendation

**Delete the custom `ResourceStamper` system entirely** — the trait,
`ResourceStamp`, `InputStamper::Custom` (the field becomes a plain
"is fs-path input" flag), the library stamper plumbing, `hash_custom`, and
both pointer-key types. Consistent with the no-backward-compat and
remove-unused-code rules; if a real custom resource ever shows up, it can
return in a simpler per-input shape informed by an actual use case. What
remains is: one `HashMap<String, FsPathId>` memoized per run, collect →
`spawn_blocking` → merge, and the existing taint/restamp path only for bound
path values. `resource/mod.rs` drops from ~380 lines to roughly 150, and the
digest fold loses its hairiest branch.

Two smaller follow-ups worth doing in the same pass:

- Merge `PreparedResourceStamps` into `RunResourceStamps` (the resolve step
  can just return one).
- Optionally hash `FsPathId` down to fixed bytes at collect time so the digest
  fold stops caring about directory structure.

Leave the eager-stamp + late-restamp architecture exactly as is — it is
earning its keep.
