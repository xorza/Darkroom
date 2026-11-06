//! The root of the UI's context chain.

use scenarium::Library;

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
/// **No document here**, deliberately: `App` composes one of these once per
/// record pass and then hands the session a `&mut` for the frame, so a
/// document reference at this level would pin the very thing the frame's
/// drains have to mutate. It enters the chain one level down, per phase.
///
/// Only shared state belongs here. The mutable sinks a frame writes through —
/// `Ui`, the intent buffer, the preferences — stay explicit parameters, so a
/// context can never be the thing two call sites fight over.
#[derive(Copy, Clone, Debug)]
pub(crate) struct AppCtx<'a> {
    theme: &'a Theme,
    library: &'a Library,
    /// Last run's centralized runtime state: per-node status/logs and the
    /// latest published values read by preview cards and viewers.
    run_state: &'a RunState,
}

impl<'a> AppCtx<'a> {
    pub(crate) const fn new(
        theme: &'a Theme,
        library: &'a Library,
        run_state: &'a RunState,
    ) -> Self {
        Self {
            theme,
            library,
            run_state,
        }
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
}
