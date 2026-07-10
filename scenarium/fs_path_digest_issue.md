# Known digest gap: Bind-delivered `FsPath` values are not content-keyed

Status: **open**. A fix existed briefly (an `fs_path` flag on `ExecutionInput` +
a digest taint) and was reverted — see "The reverted fix" below for what it did
and what it cost.

## The issue

A node's content digest folds an `FsPath` input's *external identity* — the
pointed-at file's `(len, mtime)`, or a directory's entry fingerprint — **only
when the path arrives as a `Const` binding** (`hash_fs_content` is called from
the `Const` arm of `node_digest`, `execution/digest/mod.rs`). When the path
arrives over a **`Bind` edge**, the digest folds only the producer's structural
digest. The file behind the runtime path value is invisible to every digest in
the chain, so a consumer can serve a cached output computed from file content
that no longer exists.

The digest's trust-boundary doc says "`FsPath` identity is `(len, mtime)`" —
that claim holds only for `Const` paths.

### Concrete stale-cache scenario

The only *pure* route in-tree (Watch Directory also outputs a path, but it is
`Impure`, so its consumers are digest-tainted to `None` anyway):

1. `Const path P → File Cache node → Load Image (pure, RAM/disk-cached)`.
   The File Cache node accepts the path through its `Any`-typed Value input and
   re-emits it through its wildcard output. Its own digest is its *cache-path
   string alone* (`file_cache_digest`), so neither `P`'s file content nor its
   `(len, mtime)` is folded anywhere in the chain.
2. Run once. The loader's decoded image is cached under a digest that folds
   only the passthrough's never-changing path key.
3. Overwrite the file at `P`. No digest in the chain changes, so
   `is_resident_hit` (or the disk-blob check) serves the stale decoded image
   forever.

Wired directly `Const P → loader`, the same edit re-keys via the `(len, mtime)`
fold and recomputes correctly — the File Cache hop is what launders the content
identity away. In general, *any* pure producer of `FsPath` values has this
property: a structural digest keys the path **value**, never the file behind it.

## The reverted fix

- `ExecutionInput` gained `#[serde(default)] pub fs_path: bool`, stamped at
  flatten from the func spec
  (`matches!(func_input.data_type, DataType::FsPath(_))`).
- `node_digest`'s `Bind` arm returned `None` (uncacheable) whenever the input
  was flagged: a runtime path value cannot be content-keyed structurally, so
  the consumer never caches instead of risking a stale hit.

This is sound — every Bind-delivered path is genuinely un-keyable at digest
time — but blunt: **any node with a wired path input becomes permanently
uncacheable** (RAM and disk), even when the pointed-at content never changes.
For path-plumbing pipelines (a loader fed by any upstream path output) that
erases caching for exactly the expensive nodes the cache exists for, and the
graph gives no way to opt back in.

## Directions for a real fix

- **Runtime re-key.** At input-collection time, when a `Bind` delivers an
  `FsPath` value, fold `hash_fs_path_identity(actual path)` into the consumer's
  digest before invoking it — a hybrid structural+value digest. Correct and
  precise, but it breaks the current "the whole graph resolves pre-run"
  invariant: the resolver's `Disposition::Reuse` verdict for such a node can't
  be formed until its producer's value exists, so those nodes need a
  verify-at-read step (or a `Resolved::Deferred` state) in the run loop.
- **Producer-side keying.** Require pure funcs that *emit* `FsPath` values to
  fold the emitted file's identity into their own digest. Not expressible
  today — digests are structural and outputs don't exist at digest time — so
  this reduces to the same runtime re-key, done one node earlier.
- **Accept + document.** Treat it as part of the digest's trust boundary
  (alongside "`Pure` must be pure"): a path routed through a producer serves
  stale content until some digest input changes; wire paths as consts into the
  reading node when content-keying matters. This is the current state; the
  trust-boundary note in `execution/digest/mod.rs` points here.
