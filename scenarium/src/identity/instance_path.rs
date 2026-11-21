//! [`InstancePath`]: the instances from the main graph down to one expansion of a graph.
//!
//! # Decisions
//!
//! - **An instance is named by its instance path, not by a program id.** An instance emits no
//!   program node, so the path down to it is its whole address: what a compile expands under and
//!   what a pane shows through.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::identity::NodeId;
use crate::identity::inline_ids::InlineIds;

/// The instances from the main graph down, outermost first: one expansion of a graph in the
/// program. Empty for the main graph; each id is an instance in the graph the path before it
/// ends in. A [`ProgramId`](crate::ProgramId) is a path and a node.
///
/// Held inline, as a [`ProgramId`](crate::ProgramId) is, so a pane or a run state keys on it with
/// no allocation. In a file, its list of ids, read through [`Self::checked`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<NodeId>", into = "Vec<NodeId>")]
pub struct InstancePath(InlineIds<{ Self::MAX_LEN }>);

/// Why a sequence of node ids is no [`InstancePath`].
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum InstancePathError {
    #[error("an instance path holds at most {max} ids, not {len}", max = InstancePath::MAX_LEN)]
    TooLong { len: usize },
    #[error("an instance path holds no nil id")]
    Nil,
}

impl InstancePath {
    /// The most instances a path holds: a program node at most seven instances down, so a
    /// [`ProgramId`](crate::ProgramId), its path and its node, holds eight ids, 128 bytes, and
    /// stays a plain value.
    pub const MAX_LEN: usize = 7;

    /// The path of the main graph, which nothing places.
    pub const MAIN: Self = Self(InlineIds::EMPTY);

    /// The path `ids`, outermost instance first.
    ///
    /// # Panics
    /// If `ids` is no path: see [`Self::checked`].
    pub fn new(ids: &[NodeId]) -> Self {
        match Self::checked(ids) {
            Ok(path) => path,
            Err(error) => panic!("{error}"),
        }
    }

    /// The path `ids`, outermost instance first, or why it is none: past [`Self::MAX_LEN`], or
    /// holding a nil id.
    pub fn checked(ids: &[NodeId]) -> Result<Self, InstancePathError> {
        if ids.len() > Self::MAX_LEN {
            return Err(InstancePathError::TooLong { len: ids.len() });
        }
        if ids.iter().any(|id| id.is_nil()) {
            return Err(InstancePathError::Nil);
        }
        Ok(Self(InlineIds::new(ids)))
    }

    /// The instances, outermost first.
    pub fn ids(&self) -> &[NodeId] {
        self.0.ids()
    }

    /// Whether this is the main graph's path.
    pub fn is_main(&self) -> bool {
        self.ids().is_empty()
    }

    /// The path inside `instance`, an instance in the graph this path ends in.
    ///
    /// # Panics
    /// If this path is [`Self::MAX_LEN`] long already, which the depth gate keeps an edit and a
    /// compile from reaching, or `instance` is nil.
    #[must_use]
    pub fn child(self, instance: NodeId) -> Self {
        assert!(
            self.ids().len() < Self::MAX_LEN,
            "an instance path holds at most {} ids",
            Self::MAX_LEN
        );
        assert!(!instance.is_nil(), "an instance path holds no nil id");
        Self(self.0.pushed(instance))
    }

    /// The path of the graph that places the innermost instance, or `None` for the main graph.
    pub fn parent(self) -> Option<Self> {
        let (_, outer) = self.ids().split_last()?;
        Some(Self::new(outer))
    }

    /// The innermost instance, or `None` for the main graph.
    pub fn instance(self) -> Option<NodeId> {
        self.ids().last().copied()
    }
}

impl TryFrom<Vec<NodeId>> for InstancePath {
    type Error = InstancePathError;

    fn try_from(ids: Vec<NodeId>) -> Result<Self, InstancePathError> {
        Self::checked(&ids)
    }
}

impl From<InstancePath> for Vec<NodeId> {
    fn from(path: InstancePath) -> Self {
        path.ids().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use common::SerdeFormat;

    use crate::identity::NodeId;
    use crate::identity::instance_path::{InstancePath, InstancePathError};

    fn ids(values: &[u128]) -> Vec<NodeId> {
        values.iter().copied().map(NodeId::from_u128).collect()
    }

    #[test]
    fn checked_refuses_what_is_no_path() {
        assert_eq!(InstancePath::checked(&[]), Ok(InstancePath::MAIN));
        assert_eq!(
            InstancePath::checked(&ids(&[1, 2, 3, 4, 5, 6, 7, 8])),
            Err(InstancePathError::TooLong { len: 8 })
        );
        assert_eq!(
            InstancePath::checked(&ids(&[1, 0])),
            Err(InstancePathError::Nil)
        );
        let longest = InstancePath::new(&ids(&[1, 2, 3, 4, 5, 6, 7]));
        assert_eq!(longest.ids(), ids(&[1, 2, 3, 4, 5, 6, 7]));
    }

    /// A path steps in through `child`, out through `parent`, and names a node with `node`:
    /// `[7, 3]` is the main graph's instance 7, then its compound's instance 3.
    #[test]
    fn a_path_steps_in_and_out_and_names_its_nodes() {
        let main = InstancePath::MAIN;
        assert!(main.is_main());
        assert_eq!(main.parent(), None);
        assert_eq!(main.instance(), None);

        let inner = main.child(NodeId::from_u128(7)).child(NodeId::from_u128(3));
        assert!(!inner.is_main());
        assert_eq!(inner, InstancePath::new(&ids(&[7, 3])));
        assert_eq!(inner.instance(), Some(NodeId::from_u128(3)));
        assert_eq!(inner.parent(), Some(InstancePath::new(&ids(&[7]))));
    }

    #[test]
    #[should_panic(expected = "an instance path holds at most 7 ids")]
    fn a_child_past_the_bound_panics() {
        let _ = InstancePath::new(&ids(&[1, 2, 3, 4, 5, 6, 7])).child(NodeId::from_u128(8));
    }

    /// A path is its list of ids in a file, and a read holds the list to [`InstancePath::checked`].
    #[test]
    fn a_path_round_trips_as_its_ids_and_a_bad_list_is_refused() {
        let path = InstancePath::new(&ids(&[7, 3]));
        let text = SerdeFormat::Ron.serialize(&path).unwrap();
        assert_eq!(
            text,
            SerdeFormat::Ron.serialize(&ids(&[7, 3])).unwrap(),
            "a path is written as its list"
        );
        assert_eq!(
            SerdeFormat::Ron.deserialize::<InstancePath>(&text).unwrap(),
            path
        );
        let nil = SerdeFormat::Ron.serialize(&ids(&[0])).unwrap();
        assert!(SerdeFormat::Ron.deserialize::<InstancePath>(&nil).is_err());
    }
}
