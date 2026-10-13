//! How a click on a node treats the selection it lands on.

/// Whether a click on a node replaces the selection or toggles the node in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClickMode {
    /// A plain click: the clicked node becomes the whole selection.
    Replace,
    /// A Shift-click: the clicked node joins the selection, or leaves it.
    Toggle,
}

impl ClickMode {
    /// The mode a click with Shift `held` or not asks for.
    pub(crate) const fn with_shift(held: bool) -> Self {
        if held { Self::Toggle } else { Self::Replace }
    }
}
