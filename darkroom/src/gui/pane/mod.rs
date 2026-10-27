//! The tab kinds a dock pane can show — one module per
//! [`TabRef`](crate::core::document::tab_ref::TabRef) arm, which is also one arm of the
//! match in the dock's `DockPanes::content`.
//! Adding a pane kind is a directory here and an arm there; nothing else in
//! the tree has to learn about it.

pub(crate) mod graph_ui;
pub(crate) mod image_viewer;
pub(crate) mod inspector;
pub(crate) mod preferences;
