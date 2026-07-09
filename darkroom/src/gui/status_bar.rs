//! The window's bottom status bar: a thin chrome strip reporting the runtime
//! cache's memory footprint (system RAM and GPU VRAM), mirrored from the last
//! run's `ExecutionStats`. Each pool shows only when non-zero, and the whole
//! bar is omitted when the cache holds nothing — an empty graph never grows a
//! blank strip.

use aperture::{
    Align, Background, Configure, HAlign, Panel, Sizing, Spacing, Text, TextStyle, Ui, VAlign,
};
use scenarium::data::RamUsage;

use crate::gui::app::AppContext;

const PAD_X: f32 = 8.0;
const PAD_Y: f32 = 3.0;
const FONT: f32 = 12.0;

/// Draw the bottom status bar when there's something to report. Records no
/// widget at all when the cache is empty, so the layout row collapses rather
/// than leaving a blank strip.
pub(crate) fn show(ui: &mut Ui, ctx: &AppContext<'_>) {
    let Some(label) = cache_ram_label(ctx.run_state.cache_ram) else {
        return;
    };
    let colors = &ctx.theme.colors;
    Panel::hstack()
        .id_salt("status_bar")
        .size((Sizing::FILL, Sizing::Hug))
        .child_align(Align::new(HAlign::Right, VAlign::Center))
        .padding(Spacing::xy(PAD_X, PAD_Y))
        .background(Background {
            fill: colors.chrome_fill.into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            Text::new(label)
                .style(TextStyle {
                    color: colors.text_muted,
                    font_size_px: FONT,
                    ..ui.theme.text
                })
                .show(ui);
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

/// Human-readable byte magnitude (1024-based) — the byte analogue of the node
/// header's `fmt_elapsed`: bare `B` under 1 KB, then `KB`/`MB`/`GB` carrying
/// 1–2 decimals.
fn fmt_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * KB;
    const GB: f64 = KB * KB * KB;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_bytes_steps_through_magnitudes() {
        // Sub-KB stays exact in bytes; each threshold is a power of 1024.
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1024), "1.0 KB");
        assert_eq!(fmt_bytes(1536), "1.5 KB"); // 1536 / 1024 = 1.5
        assert_eq!(fmt_bytes(1_048_576), "1.0 MB"); // 1024^2
        assert_eq!(fmt_bytes(3_145_728), "3.0 MB"); // 3 * 1024^2
        assert_eq!(fmt_bytes(1_073_741_824), "1.00 GB"); // 1024^3
        assert_eq!(fmt_bytes(1_610_612_736), "1.50 GB"); // 1.5 * 1024^3
    }

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
