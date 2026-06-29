//! The unified output cache: persist a node's outputs to a file and hydrate them
//! back so the planner prunes the node's recompute. Loading is two-staged — a cheap
//! `update`-time availability flag ([`OutputCache::mark_available`], a `stat` + codec
//! check, no bytes) that lets the planner prune, then an on-demand read of only the
//! values a run or an inspection touches ([`OutputCache::hydrate_frontier`]) — so a
//! disk-cached value behind another never enters RAM. Two policies over the one
//! storage primitive ([`blob`]), sharing the availability/load path and one store loop:
//!
//! - **content-addressed** — a `persist` node's outputs at `<disk_root>/<hex(digest)>`,
//!   keyed by its content digest. Reproducible: any upstream change re-keys it (so
//!   it's auto-invalidated), and identical computations dedup across nodes/machines.
//! - **explicit-path** — a [`CachePassthrough`](crate::special::SpecialNode) node's
//!   outputs at the `Const` `FsPath` in `input[1]`. The path *is* the key; the user
//!   manages invalidation (delete the file, or the `bypass` toggle). See
//!   `README.md` Part C.
//!
//! The two differ only in [`OutputCache::target`] (how a node maps to a file) and
//! whether a present blob is rewritten — everything else is shared.
//!
//! **Lifecycle (the call order is the contract).** The engine drives these in a
//! fixed sequence; each step depends on the previous:
//! 1. `update` → [`mark_available`](OutputCache::mark_available): flag on-disk blobs
//!    (no read), so the next plan can prune them.
//! 2. `execute`, per attempt → plan, then
//!    [`hydrate_frontier`](OutputCache::hydrate_frontier): read the frontier the
//!    schedule consumes. A failed read clears that flag and returns `false`, so the
//!    engine re-plans (the failed node then recomputes) — this is why hydration runs
//!    *after* planning and the two loop until it succeeds.
//! 3. run → [`store`](OutputCache::store): write freshly-computed outputs.
//! 4. after store → [`evict_unused`](OutputCache::evict_unused): drop RAM copies the
//!    run didn't touch but disk can serve again (must run after `store`, so a value
//!    computed this run is on disk before its RAM copy is reclaimed).
//!
//! [`hydrate_for_inspection`](OutputCache::hydrate_for_inspection) is the off-run
//! path: an editor query reads a node's value, loading its blob on demand.

use std::fmt::Write as _;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::context::ContextManager;
use crate::data::{DataType, DynamicValue};
use crate::elements::cache_passthrough::cache_node_path;
use crate::execution::blob;
use crate::execution::cache::Cache;
use crate::execution::digest::Digest;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::function::{Func, OutputType};
use crate::library::Library;
use crate::special::SpecialNode;

/// Where a node's outputs cache, and how a present blob is treated on store.
#[derive(Debug)]
enum Target {
    /// `<disk_root>/<hex(digest)>` — content-addressed: a present blob is the same
    /// bytes, so the store skips it (avoiding a redundant, possibly costly serialize).
    Addressed(PathBuf),
    /// An explicit path from `input[1]` — the store always (over)writes (a
    /// bypass/miss run rewrites).
    Explicit(PathBuf),
}

impl Target {
    fn path(&self) -> &Path {
        match self {
            Target::Addressed(p) | Target::Explicit(p) => p,
        }
    }
}

/// Persists node outputs to files and hydrates them back. Holds a snapshot of the
/// [`Library`] (its type table is the source of custom-value codecs, used by both
/// policies) and the optional content-addressed store root. Default is an empty
/// library ⇒ no codecs, `persist` nodes memory-only; `CachePassthrough` nodes
/// always use their explicit path.
///
/// The snapshot is fine to hold across library swaps: codecs are registered once
/// at assembly and never change (only subgraphs grow), so a held snapshot keeps
/// every codec it will ever need.
#[derive(Debug, Default)]
pub struct OutputCache {
    library: Arc<Library>,
    /// Root of the content-addressed store; `None` ⇒ `persist` nodes memory-only.
    disk_root: Option<PathBuf>,
}

impl OutputCache {
    /// Build a cache over `library` (its type table supplies the custom-value
    /// codecs) and an optional content-addressed store `disk_root` (`None` ⇒
    /// `persist` nodes are memory-only; `CachePassthrough` nodes always use their
    /// explicit path).
    pub fn new(library: Arc<Library>, disk_root: Option<PathBuf>) -> Self {
        Self { library, disk_root }
    }

    /// The file node `idx` caches to, or `None` when it doesn't: a
    /// `CachePassthrough` with a `Const` path → explicit; a `persist` node with a
    /// disk root and a content digest → content-addressed; anything else → none.
    fn target(&self, program: &ExecutionProgram, idx: usize, cache: &Cache) -> Option<Target> {
        let e_node = &program.e_nodes[idx];
        if matches!(e_node.special, Some(SpecialNode::CachePassthrough { .. })) {
            return cache_node_path(program.node_inputs(e_node))
                .map(|p| Target::Explicit(PathBuf::from(p)));
        }
        if e_node.persist {
            let digest = cache.current_digest(idx)?;
            return Some(Target::Addressed(
                self.disk_root.as_ref()?.join(hex(&digest)),
            ));
        }
        None
    }

    /// `update` pass: flag each cacheable node whose blob is present on disk and
    /// decodable for the current digest as
    /// [`disk_available`](crate::execution::cache::RuntimeSlot::disk_available) —
    /// **without reading it**. The planner prunes such a node's cone like any cache
    /// hit, but the bytes are deserialized only on demand (the execution frontier
    /// via [`Self::hydrate_frontier`], or an inspection read), so a disk-cached
    /// value sitting behind another disk-cached value never enters RAM. A bypassed
    /// cache node, a resident hit, a node with no current digest, or one whose
    /// outputs lack a registered codec is left unflagged (it recomputes). `library`
    /// resolves output types (the program stores spans only); the codec table read
    /// at load time is this cache's own `library`.
    pub(crate) fn mark_available(
        &self,
        program: &ExecutionProgram,
        library: &Library,
        cache: &mut Cache,
    ) {
        for idx in 0..program.e_nodes.len() {
            cache.slots[idx].disk_available = false;
            if is_bypassed(program, idx) || cache.is_resident_hit(idx) {
                continue;
            }
            if cache.current_digest(idx).is_none() {
                continue;
            }
            let Some(target) = self.target(program, idx, cache) else {
                continue;
            };
            if self.outputs_decodable(program, library, idx) && target.path().exists() {
                cache.slots[idx].disk_available = true;
            }
        }
    }

    /// Read into RAM every disk-cached value an executing node will actually
    /// consume: walk `execute_order`, and for each `Bind` input whose producer the
    /// planner serves from cache, deserialize its blob now. Producers *behind* a
    /// pruned producer are never referenced here, so a disk-cached chain loads only
    /// its frontier.
    ///
    /// Returns whether every such producer is now resident. A blob that fails to
    /// load (corrupt, deleted, undecodable) has its `disk_available` cleared by
    /// [`Self::hydrate_slot`] and makes this return `false`, so the caller re-plans:
    /// the cleared node is then scheduled to recompute rather than pruned behind a
    /// value that isn't there (which would trip the executor's "value present"
    /// invariant). Each failure clears one flag, so the re-plan loop converges.
    pub(crate) fn hydrate_frontier(
        &self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &mut Cache,
    ) -> bool {
        let mut all_loaded = true;
        for &e_idx in &plan.execute_order {
            let span = program.e_nodes[e_idx].inputs;
            for input in &program.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    // Only a producer the planner serves from cache needs loading; a
                    // producer that will execute fills its own slot during the run.
                    if plan.node_flags[addr.target_idx].cached
                        && !self.hydrate_slot(program, cache, addr.target_idx)
                    {
                        all_loaded = false;
                    }
                }
            }
        }
        all_loaded
    }

    /// Materialize node `idx` plus the producers feeding its inputs — the values an
    /// inspection of `idx` reads (its own outputs and its inputs' resolved values).
    /// Lets a disk-cached node no run touched still show its value when the editor
    /// selects it, without `mark_available` having eagerly loaded every blob.
    pub(crate) fn hydrate_for_inspection(
        &self,
        program: &ExecutionProgram,
        cache: &mut Cache,
        idx: usize,
    ) {
        self.hydrate_slot(program, cache, idx);
        let span = program.e_nodes[idx].inputs;
        for input in &program.inputs[span.range()] {
            if let ExecutionBinding::Bind(addr) = &input.binding {
                self.hydrate_slot(program, cache, addr.target_idx);
            }
        }
    }

    /// Deserialize node `idx`'s disk blob into its slot. Returns whether the slot is
    /// resident afterward: `true` if already resident or the read succeeded. On a
    /// failure — read error (codec gone, corrupt or deleted blob) *or* a blob whose
    /// types no longer match the node's declared outputs ([`Self::outputs_well_typed`])
    /// — the stale `disk_available` flag is cleared, so a re-plan recomputes the node
    /// instead of pruning it behind a value that can't be trusted, and `false` is
    /// returned.
    fn hydrate_slot(&self, program: &ExecutionProgram, cache: &mut Cache, idx: usize) -> bool {
        if cache.slots[idx].output_values.is_some() {
            return true;
        }
        if !cache.slots[idx].disk_available {
            return false;
        }
        // The slot was flagged available, so any path that fails to load it now is a
        // stale flag: clear it (`false` ⇒ the caller re-plans to recompute the node).
        if let Some(digest) = cache.current_digest(idx)
            && let Some(target) = self.target(program, idx, cache)
            && let Some(values) = blob::read(target.path(), &self.library)
            && self.outputs_well_typed(program, idx, &values)
        {
            cache.hydrate(idx, values, digest);
            return true;
        }
        cache.slots[idx].disk_available = false;
        false
    }

    /// After a run, demote resident values the run neither executed nor read as a
    /// frontier input back to disk-only — reclaiming RAM that merely duplicates a
    /// disk blob. Such a value is a prior run's leftover, pruned behind a cache this
    /// run, so nothing read it; if its blob is still on disk it's dropped from RAM
    /// and re-flagged [`disk_available`](crate::execution::cache::RuntimeSlot::disk_available),
    /// so a later run or an inspection reloads it. Lossless: a value with no blob
    /// (Memory-only, impure) is kept, so eviction never forces a recompute.
    pub(crate) fn evict_unused(
        &self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &mut Cache,
    ) {
        // Protected = nodes this run executed, plus the cached producers it read as
        // frontier inputs. Any other resident value is an untouched prior-run leftover.
        let mut protected = vec![false; program.e_nodes.len()];
        for &e_idx in &plan.execute_order {
            protected[e_idx] = true;
            let span = program.e_nodes[e_idx].inputs;
            for input in &program.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    protected[addr.target_idx] = true;
                }
            }
        }
        for (idx, &is_protected) in protected.iter().enumerate() {
            if is_protected || cache.slots[idx].output_values.is_none() {
                continue;
            }
            // Reloadable iff a blob for the current digest is on disk — only then is
            // dropping the RAM copy lossless.
            let reloadable = self
                .target(program, idx, cache)
                .is_some_and(|target| target.path().exists());
            if reloadable {
                cache.slots[idx].clear_output();
                cache.slots[idx].disk_available = true;
            }
        }
    }

    /// Whether every output of node `idx` could be decoded back from a blob: each
    /// `Custom` output type (resolved through wildcard reroutes) has a codec in this
    /// cache's library. Predicts, without reading, whether `blob::read` would
    /// succeed — so `mark_available` never flags a node whose later frontier load
    /// would fail and trip the executor's "value present" invariant. An unresolvable
    /// output type imposes no constraint (treated as decodable).
    fn outputs_decodable(&self, program: &ExecutionProgram, library: &Library, idx: usize) -> bool {
        let n_outputs = program.e_nodes[idx].outputs.len as usize;
        (0..n_outputs).all(
            |port| match effective_output_type(program, library, idx, port) {
                Some(DataType::Custom(type_id)) => self.library.codec(&type_id).is_some(),
                _ => true,
            },
        )
    }

    /// Whether decoded `values` match node `idx`'s declared output signature: the
    /// arity, plus each value's runtime kind against its declared port type (exact
    /// type id for `Custom`). A port whose type can't be resolved (partial library
    /// snapshot, or an unresolved wildcard) is not constrained. Guards against a blob
    /// whose stored types no longer match the graph — a func redefined without a
    /// version bump, or a file-cache input rewired to another type — reaching a
    /// consumer that would mis-read or panic-downcast it.
    fn outputs_well_typed(
        &self,
        program: &ExecutionProgram,
        idx: usize,
        values: &[DynamicValue],
    ) -> bool {
        if values.len() != program.e_nodes[idx].outputs.len as usize {
            return false;
        }
        values.iter().enumerate().all(|(port, value)| {
            match effective_output_type(program, &self.library, idx, port) {
                Some(ty) => value_matches_type(value, &ty),
                None => true,
            }
        })
    }

    /// Write every cacheable node that ran this round to its file, so a later run
    /// hydrates instead of recomputing. The `(target, outputs)` snapshot is taken
    /// **synchronously** (this fn isn't `async`) — the [`Cache`] isn't `Sync`, so
    /// its borrow must not cross the serialize await in the returned future; the
    /// clone is cheap (custom values are `Arc`). A content-addressed blob already
    /// on disk is skipped (same digest ⇒ same bytes — avoids re-serializing); an
    /// explicit path is always (over)written. Best-effort: a non-codec'able value
    /// is skipped, any failure logged — caching never fails a run.
    pub(crate) fn store<'a>(
        &'a self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        cache: &Cache,
        ctx: &'a mut ContextManager,
    ) -> impl Future<Output = ()> + 'a {
        let pending: Vec<(Target, Vec<DynamicValue>)> = plan
            .execute_order
            .iter()
            .filter_map(|&idx| {
                let target = self.target(program, idx, cache)?;
                // A content-addressed blob already on disk is the same bytes —
                // skip the clone (and the write) entirely, not just the serialize.
                if let Target::Addressed(path) = &target
                    && path.exists()
                {
                    return None;
                }
                Some((target, cache.output_values(idx)?.clone()))
            })
            .collect();
        async move {
            for (target, outputs) in &pending {
                if let Err(e) = blob::write(target.path(), outputs, &self.library, ctx).await {
                    tracing::warn!(path = %target.path().display(), error = %e, "failed to write output cache");
                }
            }
        }
    }
}

/// The concrete output type of node `idx`'s `port`, resolving a wildcard reroute by
/// following its mirrored input through the program's bindings. `None` when it can't
/// be resolved — the func isn't in `library` (a partial snapshot), or a wildcard's
/// mirror isn't a `Bind` (a const/unbound mirror is never a custom value). Callers
/// treat `None` as "no constraint" so a partial library never forces a false verdict.
fn effective_output_type(
    program: &ExecutionProgram,
    library: &Library,
    idx: usize,
    port: usize,
) -> Option<DataType> {
    match &node_func(program, library, idx)?.outputs.get(port)?.ty {
        OutputType::Fixed(data_type) => Some(data_type.clone()),
        OutputType::Wildcard { mirrors } => {
            let span = program.e_nodes[idx].inputs;
            match &program.inputs[span.range()].get(*mirrors)?.binding {
                ExecutionBinding::Bind(addr) => {
                    effective_output_type(program, library, addr.target_idx, addr.port_idx)
                }
                _ => None,
            }
        }
    }
}

/// Whether a runtime `value` is consistent with a declared port type. The match is
/// deliberately lenient so a legitimate value is never rejected (a false reject only
/// costs a recompute, but must not happen on the warm path): `Unbound` matches
/// anything (a port may be left unwritten), a `Static(Null)` literal is type-agnostic,
/// and a declared `Null` (unresolved wildcard) imposes nothing. The cases that matter
/// are tight: a `Custom` must carry the *exact* declared type id (a wrong-type
/// downcast is the panic this guards), and a primitive's kind must match. `Enum` is
/// matched loosely — the stored value carries only its variant name, not the type id.
fn value_matches_type(value: &DynamicValue, ty: &DataType) -> bool {
    use crate::data::StaticValue as S;
    match value {
        DynamicValue::Unbound => true,
        DynamicValue::Custom(v) => matches!(ty, DataType::Custom(tid) if v.type_id() == *tid),
        DynamicValue::Static(s) => matches!(
            (s, ty),
            (_, DataType::Null)
                | (S::Null, _)
                | (S::Float(_), DataType::Float)
                | (S::Int(_), DataType::Int)
                | (S::Bool(_), DataType::Bool)
                | (S::String(_), DataType::String)
                | (S::FsPath(_), DataType::FsPath(_))
                | (S::Enum(_), DataType::Enum(_))
        ),
    }
}

/// The func backing node `idx`: a special node's hardcoded spec, else the library
/// entry for its `func_id`. `None` when the library is a partial snapshot that
/// doesn't carry this func (so type resolution degrades to "unknown" rather than
/// panicking — see [`effective_output_type`]).
fn node_func<'a>(
    program: &'a ExecutionProgram,
    library: &'a Library,
    idx: usize,
) -> Option<&'a Func> {
    match program.e_nodes[idx].special {
        Some(special) => Some(special.func()),
        None => library.by_id(&program.e_nodes[idx].func_id),
    }
}

/// True for a bypassed cache-passthrough node — recompute + overwrite, so never
/// hydrate it.
fn is_bypassed(program: &ExecutionProgram, idx: usize) -> bool {
    matches!(
        program.e_nodes[idx].special,
        Some(SpecialNode::CachePassthrough { bypass: true })
    )
}

/// Lowercase hex of a digest — the 64-char content-addressed blob filename.
fn hex(digest: &Digest) -> String {
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_64_lowercase_chars() {
        let mut digest = [0u8; 32];
        digest[0] = 0xab;
        digest[31] = 0x0f;
        let h = hex(&digest);
        assert_eq!(h.len(), 64);
        assert!(h.starts_with("ab"));
        assert!(h.ends_with("0f"));
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    /// The value↔type matcher: tight where it must be (a `Custom` needs the exact
    /// type id — the wrong-downcast panic this guards — and a primitive's kind must
    /// match), lenient everywhere a false reject would needlessly recompute
    /// (`Unbound`, a null literal, an unresolved `Null` declared type).
    #[test]
    fn value_matches_type_is_tight_on_custom_and_kind_lenient_elsewhere() {
        use std::any::Any;
        use std::fmt;
        use std::sync::Arc;

        use crate::data::{CustomValue, StaticValue, TypeId};

        const T1: &str = "d894ee78-5d46-44cb-9671-d6816846d15f";
        const T2: &str = "12dae9d1-7cc1-400b-89a5-8e506069f8ce";

        #[derive(Debug)]
        struct V(&'static str);
        impl fmt::Display for V {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "V")
            }
        }
        impl CustomValue for V {
            fn type_id(&self) -> TypeId {
                self.0.into()
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        let int = DynamicValue::Static(StaticValue::Int(1));
        let float = DynamicValue::Static(StaticValue::Float(1.0));
        let custom_t1 = DynamicValue::Custom(Arc::new(V(T1)));

        // Primitive kinds must match exactly.
        assert!(value_matches_type(&int, &DataType::Int));
        assert!(!value_matches_type(&int, &DataType::Float));
        assert!(value_matches_type(&float, &DataType::Float));
        assert!(!value_matches_type(&float, &DataType::Int));

        // Custom must carry the exact declared type id; never crosses kinds.
        assert!(value_matches_type(&custom_t1, &DataType::Custom(T1.into())));
        assert!(!value_matches_type(
            &custom_t1,
            &DataType::Custom(T2.into())
        ));
        assert!(!value_matches_type(&custom_t1, &DataType::Int));
        assert!(!value_matches_type(&int, &DataType::Custom(T1.into())));

        // Lenient: never reject these (a false reject only costs a recompute).
        assert!(value_matches_type(
            &DynamicValue::Unbound,
            &DataType::Custom(T1.into())
        ));
        assert!(value_matches_type(&int, &DataType::Null));
        assert!(value_matches_type(
            &DynamicValue::Static(StaticValue::Null),
            &DataType::Int
        ));
    }
}
