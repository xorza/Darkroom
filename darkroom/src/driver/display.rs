//! `Display`: what the driver renders the app into. The `cpu` display, palantir's test harness,
//! is test code: `darkroom/AGENTS.md`, "The editor harness", "No `cpu` display in the
//! product".

use std::time::Duration;

use glam::UVec2;
use palantir::{
    FrameReport, GpuRequestError, HeadlessWindow, InputEvent, OffscreenTime, PowerPreference,
    RequestedGpu, Ui,
};

use crate::driver::DisplayKind;
use crate::gui::app::App;

/// The window the app runs in, by kind.
#[derive(Debug)]
pub(crate) enum Display {
    /// Each window boxed: both hold a whole `Ui`, kilobytes the enum need not carry inline. Its
    /// time moves only as the driver steps it, so animation and click timing repeat from run to
    /// run.
    Gpu(Box<HeadlessWindow>),
    #[cfg(feature = "internals")]
    #[expect(
        clippy::absolute_paths,
        reason = "a gated variant takes no gated import"
    )]
    Cpu(Box<palantir::internals::ui_harness::UiHarness>),
}

impl Display {
    /// A window of `size` physical pixels at `scale`, or the error of a GPU display on a machine
    /// with no usable adapter.
    pub(crate) fn new(kind: DisplayKind, size: UVec2, scale: f32) -> Result<Self, GpuRequestError> {
        Ok(match kind {
            DisplayKind::Gpu => {
                // No optional features: the editor draws with what every adapter has.
                #[expect(
                    clippy::default_trait_access,
                    reason = "the features are wgpu's type, which darkroom does not depend on"
                )]
                let gpu =
                    RequestedGpu::headless(PowerPreference::HighPerformance, Default::default())?;
                let window = HeadlessWindow::builder(gpu.gpu)
                    .time(OffscreenTime::Manual)
                    .open(size, scale);
                Self::Gpu(Box::new(window))
            }
            #[cfg(feature = "internals")]
            #[expect(clippy::absolute_paths, reason = "a gated arm takes no gated import")]
            DisplayKind::Cpu => Self::Cpu(Box::new(
                palantir::internals::ui_harness::UiHarness::with_text(size).scale(scale),
            )),
        })
    }

    /// Whether frames paint pixels, which a screenshot copies.
    pub(crate) const fn paints(&self) -> bool {
        matches!(self, Self::Gpu(_))
    }

    pub(crate) const fn ui(&mut self) -> &mut Ui {
        match self {
            Self::Gpu(window) => window.ui(),
            #[cfg(feature = "internals")]
            Self::Cpu(harness) => harness.ui(),
        }
    }

    /// One frame of `app`, after the clock moves by `dt`.
    pub(crate) fn frame(&mut self, app: &mut App, dt: Duration) -> FrameReport {
        self.advance(dt);
        match self {
            Self::Gpu(window) => window.frame(app),
            #[cfg(feature = "internals")]
            Self::Cpu(harness) => harness.frame_app(app),
        }
    }

    /// Move the clock by `dt` with no frame, as a pause between two clicks does.
    pub(crate) fn advance(&mut self, dt: Duration) {
        match self {
            Self::Gpu(window) => {
                window.advance(dt);
            }
            #[cfg(feature = "internals")]
            Self::Cpu(harness) => {
                harness.advance(dt);
            }
        }
    }

    pub(crate) fn on_input(&mut self, event: InputEvent<'_>) {
        match self {
            Self::Gpu(window) => {
                window.on_input(event);
            }
            #[cfg(feature = "internals")]
            Self::Cpu(harness) => {
                harness.on_input(event);
            }
        }
    }

    /// Wait until the device has finished every frame, the copies of their screenshots
    /// included, so a screenshot asked for lands in the next frame. The `cpu` display takes none.
    pub(crate) fn wait_for_gpu(&self) {
        match self {
            Self::Gpu(window) => window.wait_for_gpu(),
            #[cfg(feature = "internals")]
            Self::Cpu(_) => {}
        }
    }
}
