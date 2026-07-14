//! The window's bottom status bar: a thin chrome strip carrying the last
//! failed action's message (left, error-colored — compile/run/save/load
//! failures parked in the engine's [`StatusLog`](crate::core::status::StatusLog)
//! error slot until a subsequent success clears it) and the runtime cache's
//! memory footprint (right, system RAM and GPU VRAM mirrored from the last
//! run's `ExecutionStats`; each pool shows only when non-zero). The whole bar
//! is omitted when both are empty — an idle empty graph never grows a blank
//! strip.

use aperture::{Align, Background, Configure, HAlign, Panel, Sizing, Spacing, Text, Ui, VAlign};
use scenarium::RamUsage;

use crate::gui::app::AppContext;
use crate::gui::format::fmt_bytes;
use crate::gui::widgets::support::{colored_text, hspacer, muted_text};

const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 3.0;
const FONT: f32 = 12.0;

/// Draw the bottom status bar when there's something to report. Records no
/// widget at all when there's neither a message nor cached memory, so the
/// layout row collapses rather than leaving a blank strip.
pub(crate) fn show(ui: &mut Ui, ctx: &AppContext<'_>) {
    let ram = cache_ram_label(ctx.run_state.cache_ram);
    if ctx.status_error.is_none() && ram.is_none() {
        return;
    }
    let colors = &ctx.theme.colors;
    Panel::hstack()
        .id_salt("status_bar")
        .size((Sizing::FILL, Sizing::Hug))
        .child_align(Align::new(HAlign::Right, VAlign::Center))
        .padding(Spacing::xy(PAD_X, PAD_Y))
        .background(Background::fill(colors.chrome_fill))
        .show(ui, |ui| {
            if let Some(msg) = ctx.status_error {
                let style = colored_text(ui, colors.exec_errored_glow, FONT);
                Text::new(msg.to_string()).style(style).show(ui);
            }
            // Spacer: pins the message to the left edge and the memory
            // readout to the right.
            hspacer(ui, "status_spacer");
            if let Some(label) = ram {
                let style = muted_text(ui, ctx.theme, FONT);
                Text::new(label).style(style).show(ui);
            }
        });
}

/// The bar's label, or `None` when the cache holds nothing. A zero pool is
/// dropped, so a CPU-only cache reads `RAM 512.0 MB` with no VRAM clause and an
/// empty cache renders nothing at all.
fn cache_ram_label(ram: RamUsage) -> Option<String> {
    match (ram.cpu, ram.gpu) {
        (0, 0) => None,
        (cpu, 0) => Some(format!("RAM {}", fmt_bytes(cpu))),
        (0, gpu) => Some(format!("VRAM {}", fmt_bytes(gpu))),
        (cpu, gpu) => Some(format!("RAM {} · VRAM {}", fmt_bytes(cpu), fmt_bytes(gpu))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_ram_label_drops_zero_pools_and_empty_cache() {
        // Nothing cached → no bar.
        assert_eq!(cache_ram_label(RamUsage::default()), None);
        // A single non-zero pool shows only its own clause.
        assert_eq!(
            cache_ram_label(RamUsage { cpu: 1024, gpu: 0 }),
            Some("RAM 1.0 KB".to_string())
        );
        assert_eq!(
            cache_ram_label(RamUsage { cpu: 0, gpu: 2048 }),
            Some("VRAM 2.0 KB".to_string())
        );
        // Both present → both clauses, RAM first.
        assert_eq!(
            cache_ram_label(RamUsage {
                cpu: 1024,
                gpu: 2048
            }),
            Some("RAM 1.0 KB · VRAM 2.0 KB".to_string())
        );
    }
}
