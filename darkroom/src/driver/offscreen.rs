//! [`Offscreen`]: the editor on the driver's own thread, in palantir's [`Harness`], a window with
//! no desktop on a stepped clock.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use palantir::{
    FrameReport, GpuRequestError, Harness, InspectedWidget, PlayError, PowerPreference,
    RequestedGpu, Screenshot, Ui,
};

use crate::core::io::preferences::preferences_store::PreferencesStore;
use crate::driver::action::Play;
use crate::driver::error::DriverError;
use crate::driver::shot::{Capture, Region};
use crate::driver::{BACKGROUND_WAIT, DisplayKind, SETTLE_FRAMES, SessionConfig};
use crate::gui::app::app_host::AppHost;
use crate::gui::app::offscreen_signals::OffscreenSignals;
use crate::gui::app::{App, LaunchInputs, UnsavedEdits};
use crate::gui::dialogs::{DialogFallback, Dialogs};

/// The UI time one frame takes, as the harness's own settle steps it.
const FRAME: Duration = Duration::from_nanos(16_666_667);

/// The app, the window it runs in, and what it asked of its host.
#[derive(Debug)]
pub(crate) struct Offscreen {
    app: App,
    /// Boxed: it holds a whole `Ui`, kilobytes a driver need not carry inline.
    harness: Box<Harness>,
    signals: Arc<OffscreenSignals>,
}

impl Offscreen {
    /// An app on an empty document in a window `config` describes, or the error of a GPU display
    /// on a machine with no usable adapter.
    pub(crate) fn new(config: SessionConfig) -> Result<Self, DriverError> {
        let mut harness = Box::new(harness(config)?);
        let signals = Arc::new(OffscreenSignals::default());
        let app = launch(harness.ui(), &signals);
        Ok(Self {
            app,
            harness,
            signals,
        })
    }

    pub(crate) fn with_app<R>(&mut self, f: impl FnOnce(&mut App, &Ui) -> R) -> R {
        f(&mut self.app, self.harness.ui())
    }

    /// Play `action`, as palantir's harness plays it, settling after each step it takes.
    pub(crate) fn play(&mut self, action: &Play) -> Result<u32, DriverError> {
        self.harness
            .play(&mut self.app, std::slice::from_ref(action))
            .map_err(|failed| DriverError::from(failed.error))
    }

    pub(crate) fn widgets(&self, filter: Option<&str>) -> Vec<InspectedWidget> {
        self.harness.widgets(filter)
    }

    /// One frame. A wake asks for a full one, as the window host's `request_repaint` does: a
    /// frame nothing asked for paints only, and runs no `update` to drain what the wake announced.
    fn frame(&mut self) -> FrameReport {
        if self.signals.take_wake() {
            self.harness.ui().request_repaint();
        }
        self.harness.advance(FRAME);
        self.harness.frame_app(&mut self.app)
    }

    /// Refuse to go on once the editor asked to quit, as File ▸ Quit and Ctrl+Q do: a window
    /// closes then, and the session a driver runs ends with it.
    pub(crate) fn running(&self) -> Result<(), DriverError> {
        if self.signals.quit_asked() {
            Err(DriverError::Quit)
        } else {
            Ok(())
        }
    }

    pub(crate) fn wake_pending(&self) -> bool {
        self.signals.wake_pending()
    }

    /// Sleep until background work wakes the app or `deadline` passes.
    pub(crate) fn wait_wake(&self, deadline: Instant) {
        self.signals.wait_wake(deadline);
    }

    /// Open the absolute `document`, or a new document for none, in place of the open one, as the
    /// window's open does but discarding unsaved edits, which no one but the driver made here. An
    /// editor that quit launches again first: a window that quit is gone, and this one has no
    /// person to lose it.
    pub(crate) fn open(&mut self, document: Option<PathBuf>) -> Result<(), DriverError> {
        if self.signals.quit_asked() {
            // Signals of its own: the app before asked to quit.
            self.signals = Arc::new(OffscreenSignals::default());
            self.app = launch(self.harness.ui(), &self.signals);
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

    /// Run frames until one asks no repaint, the app has no wake pending, and no preview is
    /// converting: palantir's settle, which waits for the device while a screenshot is on its
    /// way, then the editor's own work. A timer wake, as a caret blink asks, does not count: the
    /// frame it would run changes nothing. A conversion runs on the converter's thread, so a quiet
    /// frame with one in flight waits in real time, bounded, for the wake that makes the next
    /// frame land it, and a view capture waits for the device to copy its image back, which the
    /// next frame lands. A frame that quits the editor ends the step with [`DriverError::Quit`]:
    /// every step ends here, so no call goes on past a quit, whatever it guards at its start.
    pub(crate) fn settle(&mut self, step: &str) -> Result<u32, DriverError> {
        let deadline = Instant::now() + BACKGROUND_WAIT;
        let mut busy = 0;
        for frames in 1.. {
            let report = self.frame();
            self.running()?;
            let capturing = self.app.capturing();
            if capturing {
                self.harness.wait_for_gpu();
            }
            if capturing || report.repaint_requested || self.signals.wake_pending() {
                busy += 1;
                if busy == SETTLE_FRAMES {
                    return Err(DriverError::Unsettled {
                        step: step.to_string(),
                        frames: SETTLE_FRAMES,
                    });
                }
                continue;
            }
            if !self.app.run_state().previews().in_flight() {
                return Ok(frames);
            }
            // The converter's wake announces the result; the next frame, made full, lands it.
            if !self.signals.wait_wake(deadline) {
                return Err(DriverError::UnsettledInTime {
                    step: step.to_string(),
                    seconds: BACKGROUND_WAIT.as_secs_f32(),
                });
            }
        }
        unreachable!("the frames run until the editor settles or a bound ends them")
    }
}

/// The window `config` describes, on its display's device.
fn harness(config: SessionConfig) -> Result<Harness, GpuRequestError> {
    // The harness's own settles step a frame as this driver's do.
    let builder = Harness::builder(config.size)
        .scale(config.scale)
        .frame_dt(FRAME);
    Ok(match config.display {
        DisplayKind::Gpu => {
            // No optional features: the editor draws with what every adapter has.
            #[expect(
                clippy::default_trait_access,
                reason = "the features are wgpu's type, which darkroom does not depend on"
            )]
            let gpu = RequestedGpu::headless(PowerPreference::HighPerformance, Default::default())?;
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

/// An app on an empty document, its host `signals`.
fn launch(ui: &mut Ui, signals: &Arc<OffscreenSignals>) -> App {
    App::new(
        ui,
        AppHost::Offscreen(Arc::clone(signals)),
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
