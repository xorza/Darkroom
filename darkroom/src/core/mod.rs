//! The frontend-agnostic engine: the document model + edit pipeline, the
//! evaluation worker, the scripting host, and the non-GUI `Session` the
//! `tui`/`headless` frontends drive. No Palantir, no rendering — the GUI
//! (`crate::gui`) is one consumer; `tui` / `headless` are the others. This
//! layer never imports from `crate::gui`.

pub(crate) mod document;
pub(crate) mod edit;
pub(crate) mod engine;
pub(crate) mod library;
pub(crate) mod io;
pub(crate) mod script;
pub(crate) mod session;
pub(crate) mod theme_pref;
pub(crate) mod wake;
pub(crate) mod worker;
