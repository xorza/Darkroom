//! Runtime egui debug instruments. Two independent toggles:
//!
//!  - **F11 — visual overlays**: egui's per-widget hover info,
//!    hit-test overlay, resize debug, and interactive-widgets
//!    highlight. Useful for layout/id inspection. Repaints on every
//!    pointer move (to redraw the hover overlay), so leave this OFF
//!    while measuring redraw frequency.
//!
//!  - **F12 — repaint-cause histogram**: each frame's
//!    `Context::repaint_causes()` is sampled into a rolling 1-second
//!    window. Once per second the top-5 causes are logged at info
//!    level. A runaway "request_repaint every frame" bug shows up as
//!    a single file:line dominating the list. No visual change to
//!    the UI, so the measurement is unbiased.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use egui::{Context, Key, RepaintCause};

const LOG_INTERVAL: Duration = Duration::from_secs(1);
const TOP_N: usize = 5;

#[derive(Debug)]
pub struct GuiDebug {
    overlays: bool,
    logging: bool,
    /// Frame timestamps from the last `LOG_INTERVAL` window where
    /// egui was repainting, paired with the causes it reported.
    causes_window: VecDeque<(Instant, Vec<RepaintCause>)>,
    last_log_at: Instant,
}

impl GuiDebug {
    pub fn new() -> Self {
        Self {
            overlays: false,
            logging: false,
            causes_window: VecDeque::new(),
            last_log_at: Instant::now(),
        }
    }

    /// Run once per frame.
    pub fn frame(&mut self, ctx: &Context) {
        let (toggle_overlays, toggle_logging) =
            ctx.input(|i| (i.key_pressed(Key::F11), i.key_pressed(Key::F12)));

        if toggle_overlays {
            self.overlays = !self.overlays;
            tracing::info!(enabled = self.overlays, "gui debug overlays toggled");
        }
        if toggle_logging {
            self.logging = !self.logging;
            tracing::info!(enabled = self.logging, "gui repaint logging toggled");
            if !self.logging {
                self.causes_window.clear();
            }
        }

        ctx.global_style_mut(|style| {
            style.debug.debug_on_hover = self.overlays;
            style.debug.show_widget_hits = self.overlays;
            style.debug.show_resize = self.overlays;
            style.debug.show_interactive_widgets = self.overlays;
        });

        if !self.logging {
            return;
        }

        let now = Instant::now();
        if ctx.has_requested_repaint() {
            self.causes_window.push_back((now, ctx.repaint_causes()));
        }

        let cutoff = now - LOG_INTERVAL;
        while let Some(&(t, _)) = self.causes_window.front() {
            if t < cutoff {
                self.causes_window.pop_front();
            } else {
                break;
            }
        }

        if now.duration_since(self.last_log_at) >= LOG_INTERVAL {
            self.last_log_at = now;
            self.log_histogram();
        }
    }

    fn log_histogram(&self) {
        let frames = self.causes_window.len();
        if frames == 0 {
            return;
        }

        let mut hist: HashMap<String, usize> = HashMap::new();
        for (_, causes) in &self.causes_window {
            for cause in causes {
                *hist.entry(cause.to_string()).or_default() += 1;
            }
        }

        let mut top: Vec<(String, usize)> = hist.into_iter().collect();
        top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        top.truncate(TOP_N);

        tracing::info!(
            frames_with_repaint = frames,
            top_causes = ?top,
            "gui repaint histogram (1s window)"
        );
    }
}
