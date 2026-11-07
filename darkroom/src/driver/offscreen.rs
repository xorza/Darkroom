//! [`Offscreen`]: the editor on the driver's own thread, in palantir's [`Harness`], a window with
//! no desktop on a stepped clock.

use std::path::PathBuf;
use std::time::Duration;

use glam::UVec2;
use palantir::PowerPreference;
use palantir::wgpu::Features;
use palantir::{
    AgentWindow, Depth, Gpu, Harness, HarnessSession, InspectedWidget, RequestedGpu, Ui,
};

use crate::core::io::preferences::preferences_store::PreferencesStore;
use crate::driver::action::Play;
use crate::driver::error::DriverError;
use crate::driver::{BACKGROUND_WAIT, DisplayKind, SETTLE_FRAMES, SessionConfig};
use crate::gui::app::{App, LaunchInputs, UnsavedEdits};
use crate::gui::dialogs::{DialogFallback, Dialogs};

/// The UI time one frame takes, as the harness's own settle steps it.
const FRAME: Duration = Duration::from_nanos(16_666_667);

/// The app and the window it runs in.
#[derive(Debug)]
pub(crate) struct Offscreen {
    app: App,
    /// What the window paints with, which a lost device's replacement opens again.
    display: DisplayKind,
    /// Physical pixels, which a replacement device must hold a texture of.
    size: UVec2,
    /// Boxed: it holds a whole `Ui`, kilobytes a driver need not carry inline.
    harness: Box<Harness>,
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
        let depth = match open_gpu(config.display, config.size)? {
            Some(gpu) => Depth::gpu(gpu),
            None => Depth::Record,
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
            display: config.display,
            size: config.size,
            harness,
        })
    }

    pub(crate) fn with_app<R>(&mut self, f: impl FnOnce(&mut App, &Ui) -> R) -> R {
        f(&mut self.app, self.harness.ui())
    }

    /// The harness and its app, as palantir's driver verbs take them.
    pub(crate) fn session(&mut self) -> HarnessSession<'_, App> {
        self.harness.session(&mut self.app)
    }

    /// [`Self::session`] on a device that draws: a lost one is replaced first, through the call
    /// the start made, as a window's host replaces it before its next frame. `darkroom/AGENTS.md`,
    /// "The editor harness", "A lost device is replaced offscreen too".
    pub(crate) fn painting_session(&mut self) -> Result<HarnessSession<'_, App>, DriverError> {
        if self.harness.is_device_lost() {
            let gpu =
                open_gpu(self.display, self.size)?.expect("a lost device was a GPU display's");
            self.harness.recover(gpu);
        }
        Ok(self.session())
    }

    /// Lose the window's device, as a GPU hang does; the next painting session replaces it.
    pub(crate) fn lose_device(&mut self) -> Result<(), DriverError> {
        #[cfg(feature = "internals")]
        assert!(
            !matches!(self.display, DisplayKind::TestGpu),
            "a test that loses the device takes DisplayKind::IsolatedTestGpu: the shared one is \
             every test's"
        );
        self.session()
            .lose_device()
            .map_err(|error| DriverError::played("the device loss", error))
    }

    /// Play `action`, as palantir's harness plays it, settling after each step it takes, the
    /// previews converting included ([`App::is_busy`](palantir::App::is_busy)). A step that quits
    /// the editor ends with [`DriverError::Quit`].
    pub(crate) fn play(&mut self, action: &Play) -> Result<u32, DriverError> {
        let played = self
            .painting_session()?
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

    fn quit(&self) -> bool {
        self.app.quit_flag().is_raised()
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
        let settled = self.painting_session()?.settle();
        self.running()?;
        settled.map_err(|error| DriverError::played(step, error))
    }
}

/// The device `display` paints with, opened as the start and each replacement of a lost one open
/// it, or none for a display that paints nothing; refused when the window is past what it holds.
fn open_gpu(display: DisplayKind, size: UVec2) -> Result<Option<Gpu>, DriverError> {
    let gpu = match display {
        // No optional features: the editor draws with what every adapter has.
        DisplayKind::Gpu => {
            RequestedGpu::headless(PowerPreference::HighPerformance, Features::empty())?.gpu
        }
        #[cfg(feature = "internals")]
        #[expect(clippy::absolute_paths, reason = "a gated arm takes no gated import")]
        DisplayKind::TestGpu => palantir::internals::headless_test_gpu().handles(),
        #[cfg(feature = "internals")]
        #[expect(clippy::absolute_paths, reason = "a gated arm takes no gated import")]
        DisplayKind::IsolatedTestGpu => palantir::internals::isolated_headless_test_gpu().handles(),
        #[cfg(feature = "internals")]
        DisplayKind::Cpu => return Ok(None),
    };
    check_window(size, gpu.max_texture_dim().get())?;
    Ok(Some(gpu))
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

        pub(crate) fn is_device_lost(&self) -> bool {
            self.harness.is_device_lost()
        }
    }
}
