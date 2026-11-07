//! [`Offscreen`]: the editor on the driver's own thread, in palantir's [`Harness`], a window with
//! no desktop on a stepped clock.

use std::path::PathBuf;
use std::time::Duration;

use glam::UVec2;
use palantir::PowerPreference;
use palantir::wgpu::Features;
use palantir::{
    AgentWindow, Depth, Harness, HarnessSession, InspectedWidget, RequestedGpu, Ui, WindowCommand,
};

use crate::core::io::preferences::preferences_store::PreferencesStore;
use crate::driver::action::Play;
use crate::driver::error::DriverError;
use crate::driver::{BACKGROUND_WAIT, DisplayKind, SETTLE_FRAMES, SessionConfig};
use crate::gui::MAIN_WINDOW;
use crate::gui::app::{App, LaunchInputs, UnsavedEdits};
use crate::gui::dialogs::{DialogFallback, Dialogs};

/// The UI time one frame takes, as the harness's own settle steps it.
const FRAME: Duration = Duration::from_nanos(16_666_667);

/// The app and the window it runs in.
#[derive(Debug)]
pub(crate) struct Offscreen {
    app: App,
    /// Boxed: it holds a whole `Ui`, kilobytes a driver need not carry inline.
    harness: Box<Harness>,
    /// The device a [`DisplayKind::TestGpu`] harness draws on. Declared after `harness`, as fields
    /// drop in order: the lease outlives the handles the harness holds.
    #[cfg(feature = "internals")]
    #[expect(clippy::absolute_paths, reason = "a gated field takes no gated import")]
    _test_gpu: Option<palantir::internals::HeadlessTestGpuLease>,
    /// The window commands the harness had recorded when this app launched: a close past them
    /// is this app's quit.
    launched_at: usize,
}

/// A window of `size` on a device that makes textures up to `limit` per side, which a smaller
/// adapter than a desktop's can refuse.
pub(super) fn check_window(size: UVec2, limit: u32) -> Result<(), DriverError> {
    if size.max_element() > limit {
        return Err(DriverError::WindowTooLarge { size, limit });
    }
    Ok(())
}

impl Offscreen {
    /// An app on an empty document in a window `config` describes, or the error of a GPU display
    /// on a machine with no usable adapter.
    pub(crate) fn new(config: SessionConfig) -> Result<Self, DriverError> {
        #[cfg(feature = "internals")]
        let mut test_gpu = None;
        let depth = match config.display {
            DisplayKind::Gpu => {
                // No optional features: the editor draws with what every adapter has.
                let gpu =
                    RequestedGpu::headless(PowerPreference::HighPerformance, Features::empty())?;
                check_window(config.size, gpu.gpu.max_texture_dim().get())?;
                Depth::gpu(gpu.gpu)
            }
            #[cfg(feature = "internals")]
            #[expect(clippy::absolute_paths, reason = "a gated arm takes no gated import")]
            DisplayKind::TestGpu => Depth::gpu(
                test_gpu
                    .insert(palantir::internals::headless_test_gpu())
                    .handles(),
            ),
            #[cfg(feature = "internals")]
            DisplayKind::Cpu => Depth::Record,
        };
        // The harness's own settles step a frame as this driver's do.
        let mut harness = Box::new(
            Harness::builder(config.size)
                .system_scale(config.scale)
                .frame_dt(FRAME)
                .settle_frames(SETTLE_FRAMES)
                .background_wait(BACKGROUND_WAIT)
                .depth(depth)
                .build(),
        );
        let app = launch(harness.ui_mut());
        Ok(Self {
            app,
            harness,
            #[cfg(feature = "internals")]
            _test_gpu: test_gpu,
            launched_at: 0,
        })
    }

    pub(crate) fn with_app<R>(&mut self, f: impl FnOnce(&mut App, &Ui) -> R) -> R {
        f(&mut self.app, self.harness.ui())
    }

    /// The harness and its app, as palantir's driver verbs take them.
    pub(crate) fn session(&mut self) -> HarnessSession<'_, App> {
        self.harness.session(&mut self.app)
    }

    /// Play `action`, as palantir's harness plays it, settling after each step it takes, the
    /// previews converting included ([`App::is_busy`](palantir::App::is_busy)). A step that quits
    /// the editor ends with [`DriverError::Quit`].
    pub(crate) fn play(&mut self, action: &Play) -> Result<u32, DriverError> {
        let played = self
            .session()
            .play(std::slice::from_ref(action))
            .map_err(|failed| DriverError::played(&format!("{action:?}"), failed.error));
        self.running()?;
        played
    }

    pub(crate) fn widgets(&self, filter: Option<&str>) -> Vec<InspectedWidget> {
        self.harness.widgets(filter)
    }

    /// Refuse to go on once the editor asked to quit, as File ▸ Quit and Ctrl+Q do: its window
    /// closes then, and the session a driver runs ends with it.
    pub(crate) fn running(&self) -> Result<(), DriverError> {
        if self.quit() {
            Err(DriverError::Quit)
        } else {
            Ok(())
        }
    }

    /// Whether this app closed its window.
    fn quit(&self) -> bool {
        self.harness.window_commands()[self.launched_at..]
            .iter()
            .any(
                |command| matches!(command, WindowCommand::Close(window) if *window == MAIN_WINDOW),
            )
    }

    pub(crate) fn wake_pending(&self) -> bool {
        self.harness.has_pending_wake()
    }

    /// Sleep until background work wakes the app or `timeout` passes.
    pub(crate) fn wait_wake(&self, timeout: Duration) {
        self.harness.wait_for_wake(timeout);
    }

    /// Open the absolute `document`, or a new document for none, in place of the open one, as the
    /// window's open does but discarding unsaved edits, which no one but the driver made here. An
    /// editor that quit launches again first: a window that quit is gone, and this one has no
    /// person to lose it.
    pub(crate) fn open(&mut self, document: Option<PathBuf>) -> Result<(), DriverError> {
        if self.quit() {
            self.launched_at = self.harness.window_commands().len();
            self.app = launch(self.harness.ui_mut());
        }
        match document {
            Some(document) => self.app.open_driven(&document, UnsavedEdits::Discard)?,
            None => self.app.new_driven(UnsavedEdits::Discard)?,
        }
        Ok(())
    }

    /// palantir's settle: frames until one asks no repaint, no screenshot is on its way back from
    /// the device, no wake waits and no preview converts, bounded in frames and, for a conversion,
    /// in real time. A frame that quits the editor ends the step with [`DriverError::Quit`]: every
    /// step ends here, so no call goes on past a quit, whatever it guards at its start.
    pub(crate) fn settle(&mut self, step: &str) -> Result<u32, DriverError> {
        let settled = self.session().settle();
        self.running()?;
        settled.map_err(|error| DriverError::played(step, error))
    }
}

/// An app on an empty document.
fn launch(ui: &mut Ui) -> App {
    App::new(
        ui,
        LaunchInputs {
            preferences: PreferencesStore::Memory.open(),
            document: None,
            dialogs: Dialogs::new(DialogFallback::Refuse),
        },
    )
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::driver::offscreen::Offscreen;
    use crate::gui::app::App;

    impl Offscreen {
        pub(crate) const fn app(&self) -> &App {
            &self.app
        }
    }
}
