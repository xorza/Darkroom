//! The tab kinds a dock pane can show — one module per
//! [`TabRef`](crate::core::document::TabRef) arm, which is also one arm of the
//! match in [`MainWindow::frame`](crate::gui::window::MainWindow::frame).
//! Adding a pane kind is a directory here and an arm there; nothing else in
//! the tree has to learn about it.

pub(crate) mod graph;
pub(crate) mod preferences;
pub(crate) mod viewer;
