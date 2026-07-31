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
    Align, Background, Configure, HAlign, Panel, Sizing, Spacing, Text, Ui, VAlign, WidgetId,
};
use scenarium::RamUsage;

use crate::gui::app::AppContext;
use crate::gui::format::fmt_bytes;
use crate::gui::widgets::support::{colored_text, hspacer, muted_text};

const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 3.0;
const FONT: f32 = 12.0;

/// The bar is a window singleton, so it carries a global id rather than a
/// parent-scoped salt — one strip, one name, reachable from a test.
pub(crate) fn status_bar_id() -> WidgetId {
    WidgetId::from_hash("darkroom::status_bar")
}

/// Draw the bottom status bar.
pub(crate) fn show(ui: &mut Ui, ctx: AppContext<'_>) {
    let ram = memory_label(ctx.process_memory(), ctx.run_state().cache_ram);
    let colors = &ctx.theme().colors;
    Panel::hstack()
        .id(status_bar_id())
        .size((Sizing::FILL, Sizing::HUG))
        .child_align(Align::new(HAlign::Right, VAlign::Center))
        .padding(Spacing::xy(PAD_X, PAD_Y))
        .background(Background::fill(colors.chrome_fill))
        .show(ui, |ui| {
            if let Some(msg) = ctx.status_error() {
                let style = colored_text(ui, colors.exec_errored_glow, FONT);
                Text::new(msg).style(&style).show(ui);
            }
            // Spacer: pins the message to the left edge and the memory
            // readout to the right.
            hspacer(ui, "status_spacer");
            if let Some(label) = ram {
                let style = muted_text(ui, ctx.theme(), FONT);
                Text::new(label).style(&style).show(ui);
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
/// `None` when no footprint is available, which leaves nothing worth a row.
fn memory_label(process: u64, cache: RamUsage) -> Option<String> {
    if process == 0 {
        return None;
    }
    let mut label = format!("MEM {}", fmt_bytes(process));
    let cached = cache.total();
    if cached > 0 {
        label.push_str(&format!(" · Cache {}", fmt_bytes(cached as u64)));
    }
    Some(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_label_leads_with_the_process_then_sums_both_cache_pools() {
        const MEM: u64 = 3 * 1024 * 1024;
        // An empty cache leaves the process footprint standing alone.
        assert_eq!(
            memory_label(MEM, RamUsage::default()),
            Some("MEM 3.0 MB".to_string())
        );
        // Either pool alone raises the clause on its own.
        assert_eq!(
            memory_label(MEM, RamUsage { cpu: 1024, gpu: 0 }),
            Some("MEM 3.0 MB · Cache 1.0 KB".to_string())
        );
        assert_eq!(
            memory_label(MEM, RamUsage { cpu: 0, gpu: 2048 }),
            Some("MEM 3.0 MB · Cache 2.0 KB".to_string())
        );
        // Both present → one clause carrying the sum: 1024 + 2048 = 3072 B,
        // which is exactly 3.0 KB.
        assert_eq!(
            memory_label(
                MEM,
                RamUsage {
                    cpu: 1024,
                    gpu: 2048
                }
            ),
            Some("MEM 3.0 MB · Cache 3.0 KB".to_string())
        );
        // No footprint → no readout, even with a populated cache.
        assert_eq!(
            memory_label(
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
