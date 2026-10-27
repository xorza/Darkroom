//! [`Offscreen`]: the editor on the driver's own thread, in an offscreen window on a stepped clock.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use palantir::{FrameReport, InputEvent, Rect, Ui};

use crate::core::io::preferences::Preferences;
use crate::core::io::preferences::preferences_store::PreferencesStore;
use crate::driver::display::Display;
use crate::driver::error::DriverError;
use crate::driver::shot::{Region, Shot};
use crate::driver::{BACKGROUND_WAIT, SETTLE_FRAMES, SessionConfig};
use crate::gui::app::app_host::AppHost;
use crate::gui::app::offscreen_signals::OffscreenSignals;
use crate::gui::app::{App, LaunchInputs};
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
        let app = launch(&mut display, &signals, None);
        Ok(Self {
            app,
            display,
            signals,
        })
    }

    pub(crate) fn read<R>(&mut self, f: impl FnOnce(&mut App, &Ui) -> R) -> R {
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

    /// A new app on the absolute `document`, or on an empty one for none, in place of this one,
    /// or why `document` did not open, which leaves this one as it was.
    pub(crate) fn relaunch(&mut self, document: Option<PathBuf>) -> Result<(), DriverError> {
        // Signals of its own: the app before may have asked to quit.
        let signals = Arc::new(OffscreenSignals::default());
        let app = launch(&mut self.display, &signals, document.clone());
        if let Some(document) = document {
            // A document that does not load leaves the new app on an empty one, as a person's
            // launch does; the status line holds why.
            debug_assert!(document.is_absolute(), "{}", document.display());
            if app.open_document().path.as_ref() != Some(&document) {
                return Err(DriverError::NotOpened {
                    path: document,
                    status: app.status().cur().map(|message| message.line.to_string()),
                });
            }
        }
        self.app = app;
        self.signals = signals;
        Ok(())
    }

    /// A shot of `region` as the next frame paints it; see [`crate::driver::Driver::shot`].
    pub(crate) fn shot(
        &mut self,
        region: &Region,
        max_side: u32,
        path: PathBuf,
    ) -> Result<Shot, DriverError> {
        if !self.display.paints() {
            return Err(DriverError::NoGpu);
        }
        let rect: Option<Rect> = region.rect(&self.app, self.display.ui())?;
        self.display.ui().request_screenshot(rect);
        // The frame paints the image; once the device's copy of it is back, the next frame lands
        // it, after any image an earlier ask is owed, one a frame.
        self.frame();
        for _ in 0..SETTLE_FRAMES {
            self.display.wait_for_gpu();
            self.frame();
            if let Some(screenshot) = self.display.ui().screenshot_taken() {
                // The legend reads the frame the image landed in.
                let screenshot = screenshot.clone();
                return Shot::of(&screenshot, max_side, &self.app, self.display.ui(), path);
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
            if !self.app.run_state().previews.in_flight() {
                return Ok(frames);
            }
            // The converter's wake announces the result; the next frame, made full, lands it.
            while !self.signals.wake_pending() {
                if Instant::now() > deadline {
                    return Err(DriverError::UnsettledInTime {
                        step: step.to_string(),
                        seconds: BACKGROUND_WAIT.as_secs_f32(),
                    });
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
        unreachable!("the frames run until the editor settles or a bound ends them")
    }
}

/// An app on the driver's host, opening `document` or an empty one.
fn launch(
    display: &mut Display,
    signals: &Arc<OffscreenSignals>,
    document: Option<PathBuf>,
) -> App {
    App::new(
        display.ui(),
        AppHost::Offscreen(Arc::clone(signals)),
        LaunchInputs {
            preferences: Ok(Preferences::default()),
            store: PreferencesStore::Memory,
            document,
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
