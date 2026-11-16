//! [`SubgraphSide`]: what the editor keeps beside one subgraph of the document.
//!
//! - **A linked subgraph is a copy that follows the library**: the document still holds the whole
//!   definition, so it opens anywhere, and when a scan finds the library's copy moved while this
//!   one is as it last synced, the editor updates it as one undo entry and says so. Refused: a
//!   link with no copy, as Nuke's gizmo, refused in `shared_library`.
//! - **A followed or locked subgraph is read-only**, as Blender's linked data is until made local
//!   ([link and append](https://docs.blender.org/manual/en/5.2/files/linked_libraries/link_append.html)):
//!   every edit of its graph and interface is refused with the reason; Make Local and Unlock lift
//!   it. A lock is its author's mark against an accidental edit, as a Houdini asset is locked
//!   until unlocked ([digital assets](https://www.sidefx.com/docs/houdini/assets/edit.html)), and
//!   travels with the copy. No password: a document must hold the plain graph for any build to
//!   run it, so a password would guard nothing a text editor cannot open.

use scenarium::{Graph as CoreGraph, InstancePath, SubgraphDigest};
use serde::{Deserialize, Serialize};

use crate::core::document::graph_view::GraphView;

/// What the editor keeps beside one subgraph: its view, as the main graph has one, the
/// instance its pane shows it through, and its place in the library. Kept in the document beside the scenarium subgraph, keyed
/// alike, as a view holds one placement per node: the view is the editor's, not the
/// framework's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SubgraphSide {
    pub(crate) view: GraphView,
    /// The instance whose run results the subgraph's pane shows, or `None` for none. View
    /// state, not undoable; cleared when it stops naming an instance of the subgraph.
    pub(crate) through: Option<InstancePath>,
    /// Whether the library lists this subgraph when the document is a file of the library
    /// folder.
    pub(crate) shared: bool,
    /// The digest this copy last shared with the library, by a place from it, an update or a
    /// publish; `None` for a subgraph made here. Read against the two current digests, it tells
    /// in sync, update available, changed here and changed in both apart.
    pub(crate) synced: Option<SubgraphDigest>,
    /// Follows the library: updated when the library moves and this copy has not.
    pub(crate) follows: bool,
    /// Marked read-only by its author; travels with the copy.
    pub(crate) locked: bool,
}

/// What an edit of a subgraph's place in the library writes: its marks beside the definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SideMarks {
    pub(crate) shared: bool,
    pub(crate) synced: Option<SubgraphDigest>,
    pub(crate) follows: bool,
    pub(crate) locked: bool,
}

/// Why a subgraph takes no edit of its graph or interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadOnly {
    Follows,
    Locked,
}

impl SubgraphSide {
    /// A side for `graph` with its nodes placed in a row, shown through no instance.
    pub(crate) fn new(graph: &CoreGraph) -> Self {
        Self {
            view: GraphView::new(graph),
            through: None,
            shared: false,
            synced: None,
            follows: false,
            locked: false,
        }
    }

    pub(crate) const fn marks(&self) -> SideMarks {
        SideMarks {
            shared: self.shared,
            synced: self.synced,
            follows: self.follows,
            locked: self.locked,
        }
    }

    pub(crate) const fn set_marks(&mut self, marks: SideMarks) {
        let SideMarks {
            shared,
            synced,
            follows,
            locked,
        } = marks;
        self.shared = shared;
        self.synced = synced;
        self.follows = follows;
        self.locked = locked;
    }

    /// Why the subgraph takes no edit of its graph or interface, if it takes none.
    pub(crate) const fn read_only(&self) -> Option<ReadOnly> {
        if self.follows {
            Some(ReadOnly::Follows)
        } else if self.locked {
            Some(ReadOnly::Locked)
        } else {
            None
        }
    }
}
