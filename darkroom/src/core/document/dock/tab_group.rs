//! [`TabGroup`]: one pane's tab strip.

use serde::{Deserialize, Serialize};

use crate::core::document::TabRef;
use crate::core::document::dock::TabGroupId;

/// One pane's tab strip: the open tabs plus which one is visible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct TabGroup {
    pub(crate) id: TabGroupId,
    /// Non-empty; a group whose last tab closes collapses out of the tree.
    pub(crate) tabs: Vec<TabRef>,
    /// Index of the visible tab; always in range.
    pub(crate) active: usize,
}

impl TabGroup {
    pub(crate) fn active_tab(&self) -> TabRef {
        self.tabs[self.active]
    }

    /// Remove the tab at `index`, keeping `active` on a surviving slot.
    pub(super) fn remove_tab(&mut self, index: usize) {
        self.tabs.remove(index);
        self.clamp_active();
    }

    pub(super) fn clamp_active(&mut self) {
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
    }
}
