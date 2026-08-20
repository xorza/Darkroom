//! The window's bottom status bar: a thin chrome strip carrying the last
//! failed action's message (left, error-colored — compile/run/save/load
//! failures parked in the engine's `StatusLog`
//! error slot until a subsequent success clears it) and the memory readout
//! (right): this process's own resident footprint, plus the runtime cache's
//! system + GPU bytes under one `Cache` clause, mirrored from the last
//! completed `WorkerStatus` and shown only when the cache holds something.
//! The footprint is never zero, so the bar is always present: a strip that
//! appears and vanishes reads as layout jitter, and a permanent one keeps
//! the dock from resizing when a run lands.

use palantir::{
    Align, Background, Configure, HAlign, Panel, Sizing, Spacing, Text, Ui, VAlign, WidgetId, fmt,
};
use scenarium::RamUsage;

use crate::gui::app::ctx::AppCtx;
use crate::gui::widgets::format::fmt_bytes;
use crate::gui::widgets::support::{colored_text, hspacer, muted_text};

const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 3.0;

/// The bar is a window singleton, so it carries a global id rather than a
/// parent-scoped salt — one strip, one name, reachable from a test.
pub(crate) fn status_bar_id() -> WidgetId {
    WidgetId::from_hash("darkroom::status_bar")
}

/// Draw the bottom status bar.
pub(crate) fn show(ui: &mut Ui, ctx: AppCtx<'_>) {
    let ram = MemoryLabel::resolve(ctx.process_memory(), ctx.run_state().cache_ram);
    let colors = &ctx.theme().colors;
    Panel::hstack()
        .id(status_bar_id())
        .size((Sizing::FILL, Sizing::HUG))
        .child_align(Align::new(HAlign::Right, VAlign::Center))
        .padding(Spacing::xy(PAD_X, PAD_Y))
        .background(Background::fill(colors.chrome_fill))
        .show(ui, |ui| {
            if let Some(msg) = ctx.status_error() {
                let style = colored_text(ui, ctx.theme().status.error, ctx.theme().text.body);
                Text::new(msg).style(&style).show(ui);
            }
            // Spacer: pins the message to the left edge and the memory
            // readout to the right.
            hspacer(ui, "status_spacer");
            if let Some(label) = ram {
                let style = muted_text(ui, ctx.theme(), ctx.theme().text.body);
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

impl std::fmt::Display for MemoryLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MEM {}", fmt_bytes(self.process))?;
        if self.cached > 0 {
            write!(f, " · Cache {}", fmt_bytes(self.cached))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rendered line, or `None` where there is no readout at all.
    fn rendered(process: u64, cache: RamUsage) -> Option<String> {
        MemoryLabel::resolve(process, cache).map(|label| label.to_string())
    }

    #[test]
    fn memory_label_leads_with_the_process_then_sums_both_cache_pools() {
        const MEM: u64 = 3 * 1024 * 1024;
        // An empty cache leaves the process footprint standing alone.
        assert_eq!(
            rendered(MEM, RamUsage::default()).as_deref(),
            Some("MEM 3.0 MB")
        );
        // Either pool alone raises the clause on its own.
        assert_eq!(
            rendered(MEM, RamUsage { cpu: 1024, gpu: 0 }).as_deref(),
            Some("MEM 3.0 MB · Cache 1.0 KB")
        );
        assert_eq!(
            rendered(MEM, RamUsage { cpu: 0, gpu: 2048 }).as_deref(),
            Some("MEM 3.0 MB · Cache 2.0 KB")
        );
        // Both present → one clause carrying the sum: 1024 + 2048 = 3072 B,
        // which is exactly 3.0 KB.
        assert_eq!(
            rendered(
                MEM,
                RamUsage {
                    cpu: 1024,
                    gpu: 2048
                }
            )
            .as_deref(),
            Some("MEM 3.0 MB · Cache 3.0 KB")
        );
        // No footprint → no readout, even with a populated cache.
        assert_eq!(
            rendered(
                0,
                RamUsage {
                    cpu: 1024,
                    gpu: 2048
                }
            ),
            None
        );
    }
}
