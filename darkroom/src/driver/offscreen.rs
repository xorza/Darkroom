//! [`Offscreen`]: the editor on the driver's own thread, in palantir's [`Harness`], a window with
//! no desktop on a stepped clock.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use palantir::wgpu::Features;
use palantir::{
    GpuRequestError, Harness, InspectedWidget, PlayError, PowerPreference, RequestedGpu,
    Screenshot, Ui, WindowCommand,
};

use crate::core::io::preferences::preferences_store::PreferencesStore;
use crate::driver::action::Play;
use crate::driver::error::DriverError;
use crate::driver::shot::{Capture, Region};
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
    /// The window commands the harness had recorded when this app launched: a close past them
    /// is this app's quit.
    launched_at: usize,
}

impl Offscreen {
    /// An app on an empty document in a window `config` describes, or the error of a GPU display
    /// on a machine with no usable adapter.
    pub(crate) fn new(config: SessionConfig) -> Result<Self, DriverError> {
        let mut harness = Box::new(harness(config)?);
        let app = launch(harness.ui());
        Ok(Self {
            app,
            harness,
            launched_at: 0,
        })
    }

    pub(crate) fn with_app<R>(&mut self, f: impl FnOnce(&mut App, &Ui) -> R) -> R {
        f(&mut self.app, self.harness.ui())
    }

    /// Play `action`, as palantir's harness plays it, settling after each step it takes, the
    /// previews converting included ([`App::is_busy`](palantir::App::is_busy)). A step that quits
    /// the editor ends with [`DriverError::Quit`].
    pub(crate) fn play(&mut self, action: &Play) -> Result<u32, DriverError> {
        let played = self
            .harness
            .play(&mut self.app, std::slice::from_ref(action))
            .map_err(|failed| DriverError::from(failed.error));
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

    /// Whether a wake waits for the next frame: a wait whose deadline has passed reads it at once.
    pub(crate) fn wake_pending(&self) -> bool {
        self.harness.wait_for_wake(Instant::now())
    }

    /// Sleep until background work wakes the app or `deadline` passes.
    pub(crate) fn wait_wake(&self, deadline: Instant) {
        self.harness.wait_for_wake(deadline);
    }

    /// Open the absolute `document`, or a new document for none, in place of the open one, as the
    /// window's open does but discarding unsaved edits, which no one but the driver made here. An
    /// editor that quit launches again first: a window that quit is gone, and this one has no
    /// person to lose it.
    pub(crate) fn open(&mut self, document: Option<PathBuf>) -> Result<(), DriverError> {
        if self.quit() {
            self.launched_at = self.harness.window_commands().len();
            self.app = launch(self.harness.ui());
        }
        match document {
            Some(document) => self.app.open_driven(&document, UnsavedEdits::Discard)?,
            None => self.app.new_driven(UnsavedEdits::Discard)?,
        }
        Ok(())
    }

    /// A capture of `region` as the next frame paints it; see [`crate::driver::Driver::shot`].
    pub(crate) fn shot(&mut self, region: &Region, max_side: u32) -> Result<Capture, DriverError> {
        let rect = region.rect(&self.app, self.harness.ui())?;
        let screenshot: Screenshot =
            self.harness
                .shot(&mut self.app, rect)
                .map_err(|error| match error {
                    PlayError::EmptyRegion => DriverError::EmptyRegion {
                        region: format!("{region:?}"),
                    },
                    PlayError::Unsettled { frames, .. } => DriverError::Unsettled {
                        step: format!("the shot of {region:?}"),
                        frames,
                    },
                    error => error.into(),
                })?;
        let picture = screenshot.image().fitted(max_side);
        // The legend reads the frame the image landed in, the harness's last.
        Ok(Capture::of(
            &screenshot,
            picture,
            &self.app,
            self.harness.ui(),
        ))
    }

    /// palantir's settle: frames until one asks no repaint, no screenshot is on its way back from
    /// the device, no wake waits and no preview converts, bounded in frames and, for a conversion,
    /// in real time. A frame that quits the editor ends the step with [`DriverError::Quit`]: every
    /// step ends here, so no call goes on past a quit, whatever it guards at its start.
    pub(crate) fn settle(&mut self, step: &str) -> Result<u32, DriverError> {
        let settled = self.harness.settle(&mut self.app);
        self.running()?;
        settled.map_err(|error| match error {
            PlayError::Unsettled { frames, .. } => DriverError::Unsettled {
                step: step.to_string(),
                frames,
            },
            PlayError::Busy { seconds, .. } => DriverError::UnsettledInTime {
                step: step.to_string(),
                seconds,
            },
            error => error.into(),
        })
    }
}

/// The window `config` describes, on its display's device.
fn harness(config: SessionConfig) -> Result<Harness, GpuRequestError> {
    // The harness's own settles step a frame as this driver's do.
    let builder = Harness::builder(config.size)
        .scale(config.scale)
        .frame_dt(FRAME)
        .settle_frames(SETTLE_FRAMES)
        .background_wait(BACKGROUND_WAIT);
    Ok(match config.display {
        DisplayKind::Gpu => {
            // No optional features: the editor draws with what every adapter has.
            let gpu = RequestedGpu::headless(PowerPreference::HighPerformance, Features::empty())?;
            builder.gpu(gpu.gpu).build()
        }
        #[cfg(feature = "internals")]
        #[expect(clippy::absolute_paths, reason = "a gated arm takes no gated import")]
        DisplayKind::TestGpu => builder
            .gpu(palantir::internals::headless_test_gpu().handles())
            .build(),
        #[cfg(feature = "internals")]
        DisplayKind::Cpu => builder.depth(palantir::Depth::Record).build(),
    })
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
