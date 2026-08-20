//! [`DockOp`]: the one mutation vocabulary the dock pipeline speaks, and
//! [`DockDrop`], where its move op lands a tab.

use serde::{Deserialize, Serialize};

use crate::core::document::TabRef;
use crate::core::document::dock::TabGroupId;
use crate::core::document::dock::dock_path::DockPath;
use crate::core::document::dock::split_side::SplitSide;

/// Where a moved tab lands — the payload of [`DockOp::MoveTab`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DockDrop {
    /// Join `group`'s strip at `index` (clamped to its length).
    Into { group: TabGroupId, index: usize },
    /// Split `group`'s pane; the tab becomes a fresh single-tab group on
    /// the given side.
    Split { group: TabGroupId, side: SplitSide },
}

/// One dock-layout mutation, executed by [`DockLayout::apply`](crate::core::document::dock::DockLayout::apply). The
/// single op vocabulary the whole pipeline speaks: the dock UI (or a
/// menu item, or a preview card's chip) constructs one, the frame's
/// queue transports it as `DocumentRequest::View`, and `apply` runs it.
///
/// **Every op tolerates a stale address.** One is built from a response
/// of the frame before and applied a phase later, by which time the tab,
/// group, or split it names may be gone — so an op that resolves to
/// nothing leaves the layout untouched rather than failing.
///
/// Every tab op names its tab by identity, never by strip position: an
/// index would by then address whatever tab slid into that slot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum DockOp {
    /// Make `tab` visible in whichever group holds it, and focus that
    /// group.
    ActivateTab { tab: TabRef },
    /// Open `tab` in the focused group — reusing it wherever it already
    /// sits — then make it visible and focus its pane. The whole of "show
    /// me X": the Preferences menu item and a preview card's viewer chip
    /// both raise this and nothing else.
    OpenTab { tab: TabRef },
    /// Close `tab` wherever it sits. The `Main` tab never closes — the
    /// op refuses it.
    CloseTab { tab: TabRef },
    /// Move `tab` to `to` — into another strip or splitting a pane.
    MoveTab { tab: TabRef, to: DockDrop },
    /// Set the ratio of the split at `split` (its packed root path).
    /// Emitted per frame by a divider drag; coalesces per split.
    SetRatio { split: DockPath, ratio: f32 },
    /// Move focus onto `group`, because a press landed inside its pane.
    /// The incidental half of navigation — focus following the pointer —
    /// beside the deliberate ops around it.
    FocusPane { group: TabGroupId },
}
