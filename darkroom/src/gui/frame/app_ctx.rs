//! The root of the UI's context chain.

use scenarium::Library;

use crate::core::io::preferences::run_mode::RunMode;
use crate::core::shared_library::SharedLibrary;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// The frame's read-only world, threaded down the UI tree: the theme, the
/// func library and the last run's projections.
/// `Copy` (shared refs only), so a subtree takes one parameter rather than a
/// growing fan-out of `&`s.
///
/// **The root of the context chain.** Every level below derives its own
/// context from the one above rather than restating its refs:
/// [`GraphCtx`](crate::gui::graph_ctx::GraphCtx) pairs this with the
/// document being edited, and everything under it reads the theme, the library and the run back
/// off *that*.
///
/// **No document here**, deliberately: a document reference at this level would pin what the
/// frame's drains have to mutate. It enters the chain one level down, per phase.
///
/// Only shared state belongs here. The mutable sinks a frame writes through — `Ui`, the request
/// queue, the preferences — stay explicit parameters.
#[derive(Copy, Clone, Debug)]
pub(crate) struct AppCtx<'a> {
    theme: &'a Theme,
    library: &'a Library,
    /// Last run's centralized runtime state: per-node status/logs and the
    /// latest published values read by preview cards and viewers.
    run_state: &'a RunState,
    /// The library folder's shared compounds, as the last scan read them.
    shared: &'a SharedLibrary,
    /// When a run starts unasked, a copy of the preference, so a command reads it with no
    /// borrow of the preferences the record edits.
    run_mode: RunMode,
}

impl<'a> AppCtx<'a> {
    pub(crate) const fn new(
        theme: &'a Theme,
        library: &'a Library,
        run_state: &'a RunState,
        shared: &'a SharedLibrary,
        run_mode: RunMode,
    ) -> Self {
        Self {
            theme,
            library,
            run_state,
            shared,
            run_mode,
        }
    }

    /// When a run starts unasked.
    pub(crate) const fn run_mode(self) -> RunMode {
        self.run_mode
    }

    pub(crate) const fn theme(self) -> &'a Theme {
        self.theme
    }

    /// The library every node's declaration is resolved through.
    pub(crate) const fn library(self) -> &'a Library {
        self.library
    }

    /// The last completed run's per-node verdicts and published values.
    pub(crate) const fn run_state(self) -> &'a RunState {
        self.run_state
    }

    /// The library folder's shared compounds, as the last scan read them.
    pub(crate) const fn shared(self) -> &'a SharedLibrary {
        self.shared
    }
}
