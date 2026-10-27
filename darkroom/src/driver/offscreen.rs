//! [`Offscreen`]: the editor on the driver's own thread, in an offscreen window on a stepped clock.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use palantir::{FrameReport, InputEvent, Rect, Ui};

use crate::core::io::preferences::preferences_store::PreferencesStore;
use crate::driver::display::Display;
use crate::driver::error::DriverError;
use crate::driver::shot::{Capture, Region};
use crate::driver::{BACKGROUND_WAIT, SETTLE_FRAMES, SessionConfig};
use crate::gui::app::app_host::AppHost;
use crate::gui::app::offscreen_signals::OffscreenSignals;
use crate::gui::app::{App, LaunchInputs, UnsavedEdits};
use crate::gui::dialogs::{DialogFallback, Dialogs};

/// The UI time one frame takes.
const FRAME: Duration = Duration::from_nanos(16_666_667);

/// The app, the window it renders into, and what it asked of its host.
#[derive(Debug)]
pub(crate) struct Offscreen {
    app: App,
    display: Display,
    signals: Arc<OffscreenSignals>,
}

impl Offscreen {
    /// An app on an empty document in a window `config` describes, or the error of a GPU display
    /// on a machine with no usable adapter.
    pub(crate) fn new(config: SessionConfig) -> Result<Self, DriverError> {
        let mut display = Display::new(config.display, config.size, config.scale)?;
        let signals = Arc::new(OffscreenSignals::default());
        let app = launch(&mut display, &signals);
        Ok(Self {
            app,
            display,
            signals,
        })
    }

    pub(crate) fn with_app<R>(&mut self, f: impl FnOnce(&mut App, &Ui) -> R) -> R {
        f(&mut self.app, self.display.ui())
    }

    pub(crate) fn input(&mut self, event: InputEvent<'_>) {
        self.display.on_input(event);
    }

    /// Move the clock by `dt` with no frame, as a pause between two clicks does.
    pub(crate) fn advance(&mut self, dt: Duration) {
        self.display.advance(dt);
    }

    /// One frame. A wake asks for a full one, as the window host's `request_repaint` does: a
    /// frame nothing asked for paints only, and runs no `update` to drain what the wake announced.
    pub(crate) fn frame(&mut self) -> FrameReport {
        if self.signals.take_wake() {
            self.display.ui().request_repaint();
        }
        self.display.frame(&mut self.app, FRAME)
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
            self.app = launch(&mut self.display, &self.signals);
        }
        match document {
            Some(document) => self.app.open_driven(&document, UnsavedEdits::Discard)?,
            None => self.app.new_driven(UnsavedEdits::Discard)?,
        }
        Ok(())
    }

    /// A capture of `region` as the next frame paints it; see [`crate::driver::Driver::shot`].
    pub(crate) fn shot(&mut self, region: &Region, max_side: u32) -> Result<Capture, DriverError> {
        if !self.display.paints() {
            return Err(DriverError::NoGpu);
        }
        let rect: Option<Rect> = region.rect(&self.app, self.display.ui())?;
        let Some(asked) = self.display.ui().request_screenshot(rect) else {
            return Err(DriverError::EmptyRegion {
                region: format!("{region:?}"),
            });
        };
        // The frame paints the image; once the device's copy of it is back, the next frame lands
        // it, after any image an earlier ask is owed, one a frame.
        self.frame();
        for _ in 0..SETTLE_FRAMES {
            self.display.wait_for_gpu();
            self.frame();
            let ui = self.display.ui();
            if let Some(screenshot) = ui.screenshot_taken(asked) {
                // The legend reads the frame the image landed in.
                return Ok(Capture::of(screenshot, max_side, &self.app, ui));
            }
        }
        Err(DriverError::Unsettled {
            step: format!("the shot of {region:?}"),
            frames: SETTLE_FRAMES,
        })
    }

    /// Run frames until one asks no repaint, the app has no wake pending, and no preview is
    /// converting. A timer wake, as a caret blink asks, does not count: the frame it would run
    /// changes nothing. A conversion runs on the converter's thread, so a quiet frame with one in
    /// flight waits in real time, bounded, for the wake that makes the next frame land it, and a
    /// view capture waits for the device to copy its image back, which the next frame lands. A
    /// frame that quits the editor ends the step with [`DriverError::Quit`]: every step ends
    /// here, so no call goes on past a quit, whatever it guards at its start.
    pub(crate) fn settle(&mut self, step: &str) -> Result<u32, DriverError> {
        let deadline = Instant::now() + BACKGROUND_WAIT;
        let mut busy = 0;
        for frames in 1.. {
            let report = self.frame();
            self.running()?;
            let capturing = self.app.capturing();
            if capturing {
                self.display.wait_for_gpu();
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

/// An app on the driver's host, opening `document` or an empty one.
/// An app on an empty document, its host `signals`.
fn launch(display: &mut Display, signals: &Arc<OffscreenSignals>) -> App {
    App::new(
        display.ui(),
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
