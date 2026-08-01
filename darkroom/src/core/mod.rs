//! The frontend-agnostic engine: the document model + edit pipeline and the
//! evaluation worker. No Palantir, no rendering — the GUI (`crate::gui`) is
//! its consumer, and this layer never imports from it.

mod background_runtime;
pub(crate) mod document;
pub(crate) mod edit;
pub(crate) mod io;
pub(crate) mod preview;
pub(crate) mod runtime_host;
mod runtime_library;
pub(crate) mod status;
pub(crate) mod theme_pref;
pub(crate) mod wake;
mod worker;
