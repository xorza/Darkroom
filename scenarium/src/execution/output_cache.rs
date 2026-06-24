//! The unified output cache: persist a node's outputs to a file and hydrate them
//! back so the planner prunes the node's recompute. Two policies over the one
//! storage primitive ([`blob`]), sharing one load loop and one store loop:
//!
//! - **content-addressed** — a `persist` node's outputs at `<disk_root>/<hex(digest)>`,
//!   keyed by its content digest. Reproducible: any upstream change re-keys it (so
//!   it's auto-invalidated), and identical computations dedup across nodes/machines.
//! - **explicit-path** — a [`CachePassthrough`](crate::special::SpecialNode) node's
//!   outputs at the `Const` `FsPath` in `input[1]`. The path *is* the key; the user
//!   manages invalidation (delete the file, or the `bypass` toggle). See
//!   `docs/file-cache-design.md`.
//!
//! The two differ only in [`OutputCache::target`] (how a node maps to a file) and
//! whether a present blob is rewritten — everything else is shared.

use std::fmt::Write as _;
use std::future::Future;
use std::path::{Path, PathBuf};

use crate::context::ContextManager;
use crate::data::DynamicValue;
use crate::elements::cache_passthrough::cache_node_path;
use crate::execution::blob;
use crate::execution::cache::Cache;
use crate::execution::digest::Digest;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::ExecutionProgram;
use crate::special::SpecialNode;
use crate::value_codec::CustomValueRegistry;

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

/// Persists node outputs to files and hydrates them back. Holds the *one* codec
/// registry (used by both policies) and the optional content-addressed store root.
/// Default is empty ⇒ `persist` nodes are memory-only; `CachePassthrough` nodes
/// always use their explicit path.
#[derive(Debug, Default)]
pub struct OutputCache {
    registry: CustomValueRegistry,
    /// Root of the content-addressed store; `None` ⇒ `persist` nodes memory-only.
    disk_root: Option<PathBuf>,
}

impl OutputCache {
    /// Build a cache with the codec `registry` (for custom output values) and an
    /// optional content-addressed store `disk_root` (`None` ⇒ `persist` nodes are
    /// memory-only; `CachePassthrough` nodes always use their explicit path).
    pub fn new(registry: CustomValueRegistry, disk_root: Option<PathBuf>) -> Self {
        Self {
            registry,
            disk_root,
        }
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

    /// `update` pass: hydrate each cacheable node's slot from its file, so the
    /// planner prunes the node's recompute. Skips a bypassed cache node and one
    /// already holding its file's value for the current digest (no re-decode).
    pub(crate) fn load_into(&self, program: &ExecutionProgram, cache: &mut Cache) {
        for idx in 0..program.e_nodes.len() {
            if is_bypassed(program, idx) {
                continue;
            }
            let Some(target) = self.target(program, idx, cache) else {
                continue;
            };
            if cache.is_hit(idx) {
                continue;
            }
            let Some(digest) = cache.current_digest(idx) else {
                continue;
            };
            if let Some(values) = blob::read(target.path(), &self.registry) {
                cache.hydrate(idx, values, digest);
            }
        }
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
                if let Err(e) = blob::write(target.path(), outputs, &self.registry, ctx).await {
                    tracing::warn!(path = %target.path().display(), error = %e, "failed to write output cache");
                }
            }
        }
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
}
