//! [`CacheMode`]: how a node's output is kept between runs, which a func declares for its new
//! nodes and each node carries.

use serde::{Deserialize, Serialize};

/// How a node's computed output is kept between runs: how long RAM keeps it, and whether disk
/// holds a copy. Every combination is a mode a person may want, so the two are fields of their
/// own.
///
/// This is a *storage* choice only — it never affects reproducibility. A value is kept only for a
/// node with a content digest (a reproducible cone); a node with an impure node anywhere upstream
/// has no digest, so its output is released after the run and never risks serving a stale value,
/// whatever its mode. The on-disk backend is wired only once a caller attaches a `DiskStore` with a
/// disk root; until then the disk bit degrades to memory alone.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub struct CacheMode {
    /// How long RAM keeps the output.
    pub ram: Keep,
    /// The output is persisted to the disk store, so it survives a reload; a value RAM does not
    /// keep is read back from it when a run demands it.
    pub disk: bool,
}

/// How long RAM keeps a node's output after the run that made it.
///
/// - **Every output is kept by default, until memory runs short**, as Houdini keeps each SOP's
///   cooked geometry and Nuke its buffer cache, and ComfyUI caches every node's result
///   ([caching](https://docs.comfy.org/development/comfyui-server/startup-flags)): an edit then
///   recomputes what it changed and serves the rest. The engine drops the least recently used of
///   these while the memory the process can still allocate stays under its headroom.
/// - **`Never` is Houdini's Unload flag** ([flags](https://www.sidefx.com/docs/houdini/network/flags.html)):
///   a node whose output is cheap to make, or too big to keep, recomputes when demanded.
/// - **`Always` is Nuke's cached box**: a node whose output is dear to make stays resident whatever
///   the memory, until its digest changes.
///
/// Ordered by how long each keeps, the order [`CacheMode::join`] takes the longer of.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default, PartialOrd, Ord,
)]
pub enum Keep {
    /// Dropped once the run that made it has delivered it.
    Never,
    /// Kept until memory runs short, the least recently used dropped first.
    #[default]
    Auto,
    /// Kept whatever the memory.
    Always,
}

impl CacheMode {
    /// The default, kept in RAM until memory runs short and on no disk.
    pub const AUTO: Self = Self {
        ram: Keep::Auto,
        disk: false,
    };

    /// Kept in RAM whatever the memory, on no disk.
    pub const ALWAYS: Self = Self {
        ram: Keep::Always,
        disk: false,
    };

    /// Kept nowhere: recomputed whenever a run demands it.
    pub const NEVER: Self = Self {
        ram: Keep::Never,
        disk: false,
    };

    /// Whether a current reproducible value is retained in RAM after its run and reused across
    /// runs, while memory allows or always.
    pub const fn caches_in_ram(self) -> bool {
        !matches!(self.ram, Keep::Never)
    }

    /// Whether the node's value is persisted to the disk store, so it survives a reload.
    pub const fn persists_to_disk(self) -> bool {
        self.disk
    }

    /// What both modes keep: RAM as long as either keeps it, disk where either saves it.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        Self {
            ram: self.ram.max(other.ram),
            disk: self.disk || other.disk,
        }
    }
}
