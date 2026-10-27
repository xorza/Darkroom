//! [`Display`]: what the harness renders the app into.

use std::time::Duration;

use glam::UVec2;
use palantir::internals::HeadlessWindow;
use palantir::internals::ui_harness::UiHarness;
use palantir::{FrameReport, InputEvent, PowerPreference, RequestedGpu, Ui};

use crate::gui::app::App;
use crate::internals::harness::DisplayKind;
use crate::internals::harness::stepped_clock::SteppedClock;

/// The window the app runs in, by kind.
#[derive(Debug)]
pub(crate) enum Display {
    /// Each window boxed: both hold a whole `Ui`, kilobytes the enum need not carry inline.
    Gpu {
        window: Box<HeadlessWindow>,
        clock: SteppedClock,
    },
    Cpu(Box<UiHarness>),
}

impl Display {
    /// A window of `size` physical pixels at `scale`.
    ///
    /// # Panics
    ///
    /// For [`DisplayKind::Gpu`] on a machine with no usable adapter.
    pub(crate) fn new(kind: DisplayKind, size: UVec2, scale: f32) -> Self {
        match kind {
            DisplayKind::Gpu => {
                let clock = SteppedClock::default();
                // No optional features: the editor draws with what every adapter has.
                #[expect(
                    clippy::default_trait_access,
                    reason = "the features are wgpu's type, which darkroom does not depend on"
                )]
                let gpu =
                    RequestedGpu::headless(PowerPreference::HighPerformance, Default::default())
                        .unwrap_or_else(|error| {
                            panic!("DisplayKind::Gpu needs a GPU adapter: {error}")
                        });
                let window = HeadlessWindow::builder(gpu.gpu)
                    .clock(clock.clone())
                    .open(size, scale);
                Self::Gpu {
                    window: Box::new(window),
                    clock,
                }
            }
            DisplayKind::Cpu => Self::Cpu(Box::new(UiHarness::with_text(size).scale(scale))),
        }
    }

    /// Whether frames paint pixels, which a screenshot copies.
    pub(crate) const fn paints(&self) -> bool {
        matches!(self, Self::Gpu { .. })
    }

    pub(crate) const fn ui(&mut self) -> &mut Ui {
        match self {
            Self::Gpu { window, .. } => window.ui(),
            Self::Cpu(harness) => harness.ui(),
        }
    }

    /// One frame of `app`, after the clock moves by `dt`.
    pub(crate) fn frame(&mut self, app: &mut App, dt: Duration) -> FrameReport {
        self.advance(dt);
        match self {
            Self::Gpu { window, .. } => window.frame(app),
            Self::Cpu(harness) => harness.frame_app(app),
        }
    }

    /// Move the clock by `dt` with no frame, as a pause between two clicks does.
    pub(crate) fn advance(&mut self, dt: Duration) {
        match self {
            Self::Gpu { clock, .. } => clock.advance(dt),
            Self::Cpu(harness) => {
                harness.advance(dt);
            }
        }
    }

    pub(crate) fn on_input(&mut self, event: InputEvent<'_>) {
        match self {
            Self::Gpu { window, .. } => {
                window.on_input(event);
            }
            Self::Cpu(harness) => {
                harness.on_input(event);
            }
        }
    }

    /// Give the window `size` physical pixels; the scale stays.
    pub(crate) fn resize(&mut self, size: UVec2) {
        match self {
            Self::Gpu { window, .. } => {
                let scale = window.system_scale();
                window.resize(size, scale);
            }
            Self::Cpu(harness) => {
                harness.resize(size);
            }
        }
    }
}
