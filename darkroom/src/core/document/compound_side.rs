//! [`CompoundSide`]: what the editor keeps beside one compound of the document.
//!
//! - **A linked compound is a copy that follows the library**: the document still holds the whole
//!   definition, so it opens anywhere, and when a scan finds the library's copy moved while this
//!   one is as it last synced, the editor updates it as one undo entry and says so. Refused: a
//!   link with no copy, as Nuke's gizmo, refused in `shared_library`.
//! - **A followed or locked compound is read-only**, as Blender's linked data is until made local
//!   ([link and append](https://docs.blender.org/manual/en/5.2/files/linked_libraries/link_append.html)):
//!   every edit of its graph and interface is refused with the reason; Make Local and Unlock lift
//!   it. A lock is its author's mark against an accidental edit, as a Houdini asset is locked
//!   until unlocked ([digital assets](https://www.sidefx.com/docs/houdini/assets/edit.html)), and
//!   travels with the copy. No password: a document must hold the plain graph for any build to
//!   run it, so a password would guard nothing a text editor cannot open.

use scenarium::{CompoundDigest, Graph as CoreGraph, InstancePath};
use serde::{Deserialize, Serialize};

use crate::core::document::graph_view::GraphView;

/// What the editor keeps beside one compound: its view, as the main graph has one, the
/// instance its pane shows it through, and its place in the library. Kept in the document beside the scenarium compound, keyed
/// alike, as a view holds one placement per node: the view is the editor's, not the
/// framework's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompoundSide {
    pub(crate) view: GraphView,
    /// The instance whose run results the compound's pane shows, or `None` for none. View
    /// state, not undoable; cleared when it stops naming an instance of the compound.
    pub(crate) through: Option<InstancePath>,
    /// Whether the library lists this compound when the document is a file of the library
    /// folder.
    pub(crate) shared: bool,
    /// The digest this copy last shared with the library, by a place from it, an update or a
    /// publish; `None` for a compound made here. Read against the two current digests, it tells
    /// in sync, update available, changed here and changed in both apart.
    pub(crate) synced: Option<CompoundDigest>,
    /// Follows the library: updated when the library moves and this copy has not.
    pub(crate) follows: bool,
    /// Marked read-only by its author; travels with the copy.
    pub(crate) locked: bool,
}

/// What an edit of a compound's place in the library writes: its marks beside the definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SideMarks {
    pub(crate) shared: bool,
    pub(crate) synced: Option<CompoundDigest>,
    pub(crate) follows: bool,
    pub(crate) locked: bool,
}

/// Why a compound takes no edit of its graph or interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadOnly {
    Follows,
    Locked,
}

impl CompoundSide {
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

    /// Why the compound takes no edit of its graph or interface, if it takes none.
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
