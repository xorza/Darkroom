//! The frontend-agnostic engine: the document model, the edit pipeline and
//! the evaluation worker. The GUI (`crate::gui`) is its consumer, and this
//! layer never imports from it.
//!
//! It renders nothing, but it stores palantir's data types, because the document and the
//! preferences save them: the pane arrangement is a [`palantir::DockState`], edited through
//! [`palantir::DockOperation`], the viewer's sampling preference a [`palantir::ImageFilter`], the
//! interface size a `UserScale`. Nothing that draws is used here.

pub(crate) mod document;
pub(crate) mod edit;
pub(crate) mod io;
pub(crate) mod open_document;
pub(crate) mod preview_sink;
pub(crate) mod runtime_host;
pub(crate) mod runtime_library;
pub(crate) mod status_log;
pub(crate) mod wake;
pub(crate) mod worker_bridge;
