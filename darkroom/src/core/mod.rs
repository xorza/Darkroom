//! The frontend-agnostic engine: the document model, the edit pipeline and
//! the evaluation worker. The GUI (`crate::gui`) is its consumer, and this
//! layer never imports from it.
//!
//! It renders nothing, but it stores two palantir data types, because the
//! document saves them: the pane arrangement is a [`palantir::DockState`],
//! edited through [`palantir::DockOp`], and the viewer's sampling preference
//! is a [`palantir::ImageFilter`]. Nothing else from palantir is used here.

mod background_runtime;
pub(crate) mod document;
pub(crate) mod edit;
pub(crate) mod io;
pub(crate) mod preview;
pub(crate) mod runtime_host;
mod runtime_library;
pub(crate) mod status;
pub(crate) mod wake;
mod worker;
