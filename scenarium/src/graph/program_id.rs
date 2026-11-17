//! [`ProgramId`]: the id of a program node, which is its path.
//!
//! # Decisions
//!
//! - **A program node's id is its path**: the instances from the main graph down, then the node.
//!   It is unique by construction, the same on every compile, and reads back as the place to show;
//!   in the program's sorted order one instance's nodes are one run. As ComfyUI's execution id, "a
//!   list of all subgraph node IDs, followed by the actual original node ID"
//!   ([subgraphs](https://docs.comfy.org/custom-nodes/js/subgraphs)). Refused: a hash of the path,
//!   as Blender's compute context hash
//!   ([`BLI_compute_context.hh`](https://projects.blender.org/archive/blender-archive/src/commit/5cc793912efe4432b51cc7ddda4005c56fcbc93b/source/blender/blenlib/BLI_compute_context.hh)),
//!   which the program would hold beside the path it still needs to name a failed node; a root
//!   node's own id beside hashed inner ones, which a file can make collide.
//! - **A program id is a value of its own, its path held inline**, bounded by
//!   [`ProgramShape::MAX_LEVELS`](crate::ProgramShape::MAX_LEVELS), so reports, seeds, evictions
//!   and viewer tabs carry it with no allocation. It sits with the authored addresses, so the
//!   authoring model imports nothing of the execution side.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::graph::identity::NodeId;
use crate::graph::instance_path::InstancePath;

/// The id of a program node: its path, the instances from the main graph down and then the node
/// itself. Unique by construction, as a node id is unique inside its graph and each instance
/// expands its compound once; the same on every compile; and, ordered element by element, one
/// instance's nodes are one run of a sorted program. As ComfyUI's execution id, "a list of all
/// subgraph node IDs, followed by the actual original node ID"
/// ([subgraphs](https://docs.comfy.org/custom-nodes/js/subgraphs)).
///
/// Held inline, so a report, a seed or an eviction carries it with no allocation. Refused: a
/// hash of the path, as Blender's compute context hash, which the program would hold beside the
/// path it still needs to name a failed node. In a file, its list of ids, read through
/// [`Self::checked`].
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(try_from = "Vec<NodeId>", into = "Vec<NodeId>")]
pub struct ProgramId {
    len: u8,
    /// The path in `..len`; the tail is nil, so no two equal paths differ in memory.
    ids: [NodeId; Self::MAX_LEN],
}

/// Why a sequence of node ids is no [`ProgramId`].
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ProgramIdError {
    #[error("a program id names at least its node")]
    Empty,
    #[error("a program id holds at most {max} ids, not {len}", max = ProgramId::MAX_LEN)]
    TooLong { len: usize },
    #[error("a program id holds no nil id")]
    Nil,
}

impl ProgramId {
    /// The most ids a path holds: a program node at most seven instances down. Eight, as a path
    /// of eight ids is 128 bytes and stays a plain value.
    pub const MAX_LEN: usize = 8;

    /// The path `ids`, outermost instance first.
    ///
    /// # Panics
    /// If `ids` is no path: see [`Self::checked`].
    pub fn new(ids: &[NodeId]) -> Self {
        match Self::checked(ids) {
            Ok(program_id) => program_id,
            Err(error) => panic!("{error}"),
        }
    }

    /// The path `ids`, outermost instance first, or why it is none: empty, past
    /// [`Self::MAX_LEN`], or holding a nil id.
    pub fn checked(ids: &[NodeId]) -> Result<Self, ProgramIdError> {
        if ids.is_empty() {
            return Err(ProgramIdError::Empty);
        }
        if ids.len() > Self::MAX_LEN {
            return Err(ProgramIdError::TooLong { len: ids.len() });
        }
        if ids.iter().any(|id| id.is_nil()) {
            return Err(ProgramIdError::Nil);
        }
        let mut program_id = Self {
            len: ids.len() as u8,
            ids: [NodeId::nil(); Self::MAX_LEN],
        };
        program_id.ids[..ids.len()].copy_from_slice(ids);
        Ok(program_id)
    }

    /// The path, outermost instance first, the node last.
    pub fn ids(&self) -> &[NodeId] {
        &self.ids[..usize::from(self.len)]
    }

    /// The authored node this program node is, in the graph of its innermost instance.
    pub fn node_id(&self) -> NodeId {
        self.ids()[usize::from(self.len) - 1]
    }

    /// The instances the node lies inside, outermost first: the main graph's path for a node of
    /// the main graph.
    pub fn path(&self) -> InstancePath {
        InstancePath::new(&self.ids()[..usize::from(self.len) - 1])
    }

    /// The path as bytes of a fixed length: the count of ids, then [`Self::MAX_LEN`] ids of 16
    /// bytes each, the unused ones nil. What a disk blob holds to check it is the node's.
    pub(crate) fn to_bytes(self) -> [u8; Self::BYTES_LEN] {
        let mut bytes = [0; Self::BYTES_LEN];
        bytes[0] = self.len;
        let (chunks, _) = bytes[1..].as_chunks_mut::<16>();
        for (chunk, id) in chunks.iter_mut().zip(self.ids) {
            chunk.copy_from_slice(&id.as_u128().to_le_bytes());
        }
        bytes
    }

    /// The length of [`Self::to_bytes`].
    pub(crate) const BYTES_LEN: usize = 1 + 16 * Self::MAX_LEN;
}

impl TryFrom<Vec<NodeId>> for ProgramId {
    type Error = ProgramIdError;

    fn try_from(ids: Vec<NodeId>) -> Result<Self, ProgramIdError> {
        Self::checked(&ids)
    }
}

impl From<ProgramId> for Vec<NodeId> {
    fn from(program_id: ProgramId) -> Self {
        program_id.ids().to_vec()
    }
}

impl From<NodeId> for ProgramId {
    /// The id of a node of the main graph.
    ///
    /// # Panics
    /// If `node_id` is nil.
    fn from(node_id: NodeId) -> Self {
        Self::new(&[node_id])
    }
}

impl PartialEq for ProgramId {
    fn eq(&self, other: &Self) -> bool {
        self.ids() == other.ids()
    }
}

impl Eq for ProgramId {}

impl PartialOrd for ProgramId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProgramId {
    /// Element by element, a path before every longer path it begins: so the nodes inside one
    /// instance sort together.
    fn cmp(&self, other: &Self) -> Ordering {
        self.ids().cmp(other.ids())
    }
}

impl Hash for ProgramId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ids().hash(state);
    }
}

impl fmt::Debug for ProgramId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ProgramId").field(&self.ids()).finish()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use common::SerdeFormat;

    use crate::graph::identity::NodeId;
    use crate::graph::instance_path::InstancePath;
    use crate::graph::program_id::{ProgramId, ProgramIdError};

    fn ids(values: &[u128]) -> Vec<NodeId> {
        values.iter().copied().map(NodeId::from_u128).collect()
    }

    #[test]
    fn checked_refuses_what_is_no_path() {
        assert_eq!(ProgramId::checked(&[]), Err(ProgramIdError::Empty));
        assert_eq!(
            ProgramId::checked(&ids(&[1, 2, 3, 4, 5, 6, 7, 8, 9])),
            Err(ProgramIdError::TooLong { len: 9 })
        );
        assert_eq!(ProgramId::checked(&ids(&[1, 0])), Err(ProgramIdError::Nil));
        let longest = ProgramId::new(&ids(&[1, 2, 3, 4, 5, 6, 7, 8]));
        assert_eq!(longest.ids(), ids(&[1, 2, 3, 4, 5, 6, 7, 8]));
    }

    /// An id splits into the path of the graph its node lies in and the node, and the two build it
    /// again.
    #[test]
    fn an_id_splits_into_its_path_and_its_node() {
        let root = ProgramId::from(NodeId::from_u128(7));
        assert_eq!(root.ids(), ids(&[7]));
        assert_eq!(root.path(), InstancePath::MAIN);
        assert_eq!(root.node_id(), NodeId::from_u128(7));

        let inner = ProgramId::new(&ids(&[7, 3, 9]));
        assert_eq!(inner.path(), InstancePath::new(&ids(&[7, 3])));
        assert_eq!(inner.node_id(), NodeId::from_u128(9));
        assert_eq!(inner.path().node(inner.node_id()), inner);
    }

    /// The order is the paths' element by element, so an instance's nodes are one run: `[1]`,
    /// `[1, 1]`, `[1, 5]`, `[2]` — the nodes inside instance 1 sit between it and node 2, as
    /// `[1, 5] < [2]` compares 1 against 2 first. A field-order derive would compare the lengths
    /// first and put `[2]` before `[1, 1]`.
    #[test]
    fn an_instances_nodes_sort_together() {
        let mut sorted = vec![
            ProgramId::new(&ids(&[2])),
            ProgramId::new(&ids(&[1, 5])),
            ProgramId::new(&ids(&[1])),
            ProgramId::new(&ids(&[1, 1])),
        ];
        sorted.sort();
        assert_eq!(
            sorted,
            [
                ProgramId::new(&ids(&[1])),
                ProgramId::new(&ids(&[1, 1])),
                ProgramId::new(&ids(&[1, 5])),
                ProgramId::new(&ids(&[2])),
            ]
        );
    }

    #[test]
    fn equal_paths_hash_alike() {
        let hash = |program_id: ProgramId| {
            let mut hasher = DefaultHasher::new();
            program_id.hash(&mut hasher);
            hasher.finish()
        };
        let built = InstancePath::MAIN
            .child(NodeId::from_u128(1))
            .node(NodeId::from_u128(2));
        let given = ProgramId::new(&ids(&[1, 2]));
        assert_eq!(built, given);
        assert_eq!(hash(built), hash(given));
        assert_ne!(built, ProgramId::new(&ids(&[1])));
    }

    /// An id is its list of ids in a file, and a read holds the list to [`ProgramId::checked`].
    #[test]
    fn an_id_round_trips_as_its_ids_and_a_bad_list_is_refused() {
        let program_id = ProgramId::new(&ids(&[7, 3]));
        let text = SerdeFormat::Ron.serialize(&program_id).unwrap();
        assert_eq!(text, SerdeFormat::Ron.serialize(&ids(&[7, 3])).unwrap());
        assert_eq!(
            SerdeFormat::Ron.deserialize::<ProgramId>(&text).unwrap(),
            program_id
        );
        let empty = SerdeFormat::Ron.serialize(&ids(&[])).unwrap();
        assert!(SerdeFormat::Ron.deserialize::<ProgramId>(&empty).is_err());
    }
}
