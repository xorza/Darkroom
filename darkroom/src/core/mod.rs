//! The frontend-agnostic engine: the document model + edit pipeline, the
//! evaluation worker, the scripting host, and the non-GUI `TerminalSession` the
//! `tui`/`headless` frontends drive. No Aperture, no rendering — the GUI
//! (`crate::gui`) is one consumer; `tui` / `headless` are the others. This
//! layer never imports from `crate::gui`.

pub(crate) mod background_runtime;
pub(crate) mod document;
pub(crate) mod edit;
pub(crate) mod graph_library;
pub(crate) mod io;
pub(crate) mod runtime_library;
pub(crate) mod runtime_host;
pub(crate) mod script;
pub(crate) mod status;
pub(crate) mod terminal_session;
pub(crate) mod theme_pref;
pub(crate) mod wake;
pub(crate) mod worker;
pub(crate) mod workspace;
