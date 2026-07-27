//! The frontend-agnostic engine: the document model + edit pipeline, the
//! evaluation worker, the scripting host, and the non-GUI `TerminalSession` the
//! `tui`/`headless` frontends drive. No Palantir, no rendering — the GUI
//! (`crate::gui`) is one consumer; `tui` / `headless` are the others. This
//! layer never imports from `crate::gui`.

mod background_runtime;
pub(crate) mod document;
pub(crate) mod edit;
mod graph_library;
pub(crate) mod io;
pub(crate) mod preview;
mod runtime_host;
mod runtime_library;
pub(crate) mod script;
mod status;
pub(crate) mod terminal_session;
pub(crate) mod theme_pref;
pub(crate) mod wake;
mod worker;
pub(crate) mod workspace;
