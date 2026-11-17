//! [`PaneKind`]: a pane a menu shows by name.

use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::view_target::ViewTarget;

/// A pane a menu shows by name, one tab at most each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PaneKind {
    Viewer,
    Inspector,
    Preferences,
    /// The commands and the canvas's gestures, each with its keys.
    Reference,
}

impl PaneKind {
    /// The name the kind's menu row and its tab show.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Viewer => "Viewer",
            Self::Inspector => "Inspector",
            Self::Preferences => "Preferences",
            Self::Reference => "Keyboard and Mouse",
        }
    }

    /// The tab the kind shows: the viewer that follows the active preview, the inspector that
    /// follows the selection.
    pub(crate) const fn tab(self) -> TabRef {
        match self {
            Self::Viewer => TabRef::Viewer(ViewTarget::Active),
            Self::Inspector => TabRef::Inspector(InspectTarget::Selection),
            Self::Preferences => TabRef::Preferences,
            Self::Reference => TabRef::Reference,
        }
    }
}
