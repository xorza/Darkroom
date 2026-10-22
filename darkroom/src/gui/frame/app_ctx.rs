//! The root of the UI's context chain.

use scenarium::Library;

use crate::core::status_log::StatusMessage;

use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// The frame's read-only world, threaded down the UI tree: the theme, the
/// func library, the last run's projections, and the status-bar inputs.
/// `Copy` (shared refs only), so a subtree takes one parameter rather than a
/// growing fan-out of `&`s.
///
/// **The root of the context chain.** Every level below derives its own
/// context from the one above rather than restating its refs:
/// [`WindowCtx`](crate::gui::frame::window_ctx::WindowCtx) pairs this with the
/// document being edited, [`GraphCtx`](crate::gui::graph_ctx::GraphCtx) adds
/// whether a pane shows the graph, and everything under the canvas reads the theme, the library and the run back off *that* — which is why
/// nothing below [`MainWindow`](crate::gui::main_window::MainWindow) names this
/// type.
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
    status: StatusInputs<'a>,
}

impl<'a> AppCtx<'a> {
    /// The frame's world with no status-bar inputs: no error, no memory
    /// figure — see [`Self::with_status`].
    pub(crate) const fn new(
        theme: &'a Theme,
        library: &'a Library,
        run_state: &'a RunState,
    ) -> Self {
        Self {
            theme,
            library,
            run_state,
            status: StatusInputs {
                message: None,
                process_memory: 0,
            },
        }
    }

    /// This world with the status bar reading `status`.
    pub(crate) const fn with_status(self, status: StatusInputs<'a>) -> Self {
        Self { status, ..self }
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

    /// The last failure or warning (the engine's
    /// [`StatusLog::current`](crate::core::status_log::StatusLog::current)),
    /// shown in the status bar until a success of its family clears it.
    pub(crate) const fn status_message(self) -> Option<StatusMessage<'a>> {
        self.status.message
    }

    /// This process's resident bytes (see
    /// [`ProcessMemory`](crate::gui::state::process_memory::ProcessMemory)), rendered as the
    /// status bar's `MEM` clause.
    pub(crate) const fn process_memory(self) -> u64 {
        self.status.process_memory
    }
}

/// The two status-bar inputs `App` owns, bundled because they travel
/// together and nothing between here and the bar reads either.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct StatusInputs<'a> {
    pub(crate) message: Option<StatusMessage<'a>>,
    pub(crate) process_memory: u64,
}
