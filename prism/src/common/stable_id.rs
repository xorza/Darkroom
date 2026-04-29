//! Widget id salting for egui persistent IDs.
//!
//! Every widget that needs a stable identity across frames takes a
//! [`StableId`]. Our own scope primitives (`Gui::scope`, `Button::show`,
//! `Frame::show`, etc.) accept *only* `StableId`, which makes id
//! provenance greppable: any `StableId` in the tree came from either a
//! [`StableId::new`] call (call-site-salted) or a [`StableId::with`]
//! derivation off a parent `StableId`.
//!
//! For list items or per-instance widgets, pass a tuple that includes
//! the runtime key:
//!
//! ```ignore
//! Button::default().show(gui, StableId::new("cache"));
//! Button::default().show(gui, StableId::new(("cache_btn", node.id)));
//! Button::default().show(gui, parent.with("scroll"));
//! ```

use std::hash::Hash;

use egui::Id;

/// A widget id pinned to its construction site.
///
/// The wrapped [`Id`] is used verbatim as a child Ui's registered widget
/// id via `UiBuilder::id(...)` (global_scope=true) — bypassing egui's
/// default `unique_id = stable_id.with(parent_counter)` formula, which
/// drifts whenever conditional siblings come and go in the parent Ui
/// and triggers "widget rect changed id between passes" warnings on
/// our fixed-rect chrome.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StableId(Id);

impl StableId {
    /// Build an id rooted at the call site (`#[track_caller]`) plus
    /// the caller-supplied `name`. The name can be a literal string, a
    /// runtime key, or a tuple composing both (the common list-item
    /// pattern: `StableId::new(("func_btn", func.id))`).
    #[track_caller]
    pub fn new(name: impl Hash) -> Self {
        let loc = std::panic::Location::caller();
        Self(Id::new((loc.file(), loc.line(), name)))
    }

    /// Derive a child id from this one. Use when a widget needs a
    /// subordinate id (e.g. the scroll region inside a status panel)
    /// and already has a parent `StableId` — keeps us inside the
    /// wrapper instead of round-tripping through a raw `egui::Id`.
    pub fn with(self, salt: impl Hash) -> Self {
        Self(self.0.with(salt))
    }

    /// Access the underlying [`Id`]. Needed for interop with egui APIs
    /// that don't flow through our wrappers (e.g. `UiBuilder::id`
    /// directly, or `make_persistent_id` call sites).
    pub fn id(self) -> Id {
        self.0
    }
}
