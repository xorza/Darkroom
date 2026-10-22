//! The window's bottom status bar: a thin chrome strip carrying the last
//! failed action's message or a warning (left, in the error or the warning
//! color — run/save/load failures and what a load repaired, parked in the
//! engine's `StatusLog` until a success of the same family clears it) and the memory readout (right): this process's own resident footprint, plus the runtime cache's
//! system + GPU bytes under one `Cache` clause, mirrored from the last
//! completed `RunSummary` and shown only when the cache holds something.
//! The footprint is never zero, so the bar is always present: a strip that
//! appears and vanishes reads as layout jitter, and a permanent one keeps
//! the dock from resizing when a run lands.

use palantir::prelude::*;
use scenarium::RamUsage;

use crate::core::status_log::Severity;
use crate::gui::frame::app_ctx::AppCtx;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::support;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 3.0;

/// The bar is a window singleton, so it carries a global id rather than a
/// parent-scoped salt — one strip, one name, reachable from a test.
pub(crate) fn id() -> WidgetId {
    WidgetId::from_hash("darkroom::status_bar")
}

/// Draw the bottom status bar.
pub(crate) fn show(ui: &mut Ui, ctx: AppCtx<'_>) {
    let ram = MemoryLabel::resolve(ctx.process_memory(), ctx.run_state().cache_ram);
    let colors = &ctx.theme().colors;
    Panel::hstack()
        .id(id())
        .size((Sizing::FILL, Sizing::HUG))
        .child_align(Align::new(HAlign::Right, VAlign::Center))
        .padding(Spacing::xy(PAD_X, PAD_Y))
        .background(Background::fill(colors.chrome_fill))
        .show(ui, |ui| {
            if let Some(message) = ctx.status_message() {
                let status = &ctx.theme().status;
                let color = match message.severity {
                    Severity::Error => status.error,
                    Severity::Warning => status.warning,
                };
                let style = support::colored_text(ui, color, ctx.theme().text.body);
                Text::new(message.line).style(&style).show(ui);
            }
            // Spacer: pins the message to the left edge and the memory
            // readout to the right.
            support::hspacer(ui, "status_spacer");
            if let Some(label) = ram {
                let style = support::muted_text(ui, ctx.theme(), ctx.theme().text.body);
                Text::new(fmt!(ui, "{label}")).style(&style).show(ui);
            }
        });
}

/// The bar's memory readout: this process's footprint, plus a `Cache`
/// clause — the runtime cache's system and GPU pools summed — whenever the
/// cache holds anything at all.
///
/// The two figures overlap on purpose: the cache's system half is also
/// inside the process footprint. They answer different questions — `MEM` is
/// what this process costs the machine, `Cache` is how much of that is
/// retained node results, which is the half a cache eviction can give back.
///
/// `Display` rather than an assembled `String`: the bar records every frame
/// and the whole line goes straight into the record pass's text arena, which
/// is where the two byte figures were headed anyway.
#[derive(Clone, Copy, Debug)]
struct MemoryLabel {
    /// This process's resident footprint. Never zero — a zero one is what
    /// [`Self::resolve`] answers `None` to.
    process: u64,
    /// The runtime cache's two pools summed. Zero drops the clause.
    cached: u64,
}

impl MemoryLabel {
    /// The readout for one frame's figures, or `None` when no footprint is
    /// available — which leaves nothing worth a row.
    fn resolve(process: u64, cache: RamUsage) -> Option<Self> {
        (process != 0).then(|| Self {
            process,
            cached: cache.total() as u64,
        })
    }
}

impl Display for MemoryLabel {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "MEM {}", Bytes::new(self.process))?;
        if self.cached > 0 {
            write!(f, " · Cache {}", Bytes::new(self.cached))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
