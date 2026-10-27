use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::gui::app::session::internals::SessionHarness;
use crate::gui::theme::Theme;

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
        Some("MEM 3.0 MiB")
    );
    // Either pool alone raises the clause on its own.
    assert_eq!(
        rendered(MEM, RamUsage { cpu: 1024, gpu: 0 }).as_deref(),
        Some("MEM 3.0 MiB · Cache 1.0 KiB")
    );
    assert_eq!(
        rendered(MEM, RamUsage { cpu: 0, gpu: 2048 }).as_deref(),
        Some("MEM 3.0 MiB · Cache 2.0 KiB")
    );
    // Both present → one clause carrying the sum: 1024 + 2048 = 3072 B,
    // which is exactly 3.0 KiB.
    assert_eq!(
        rendered(
            MEM,
            RamUsage {
                cpu: 1024,
                gpu: 2048
            }
        )
        .as_deref(),
        Some("MEM 3.0 MiB · Cache 3.0 KiB")
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

/// The process footprint gives the bar something to say on every frame,
/// so it is recorded on an untouched document — and stays recorded when no
/// reading is available, rather than reappearing as the figure lands.
#[test]
fn status_bar_is_recorded_on_an_idle_document_with_or_without_a_reading() {
    let mut h = SessionHarness::new(DocumentFixture::default());
    h.prime(2);
    let without =
        h.ui.rect(id())
            .expect("status bar records with no reading and an empty cache");

    h.process_memory = 3 * 1024 * 1024;
    h.prime(2);
    let with = h.ui.rect(id()).expect("status bar records");

    // Padding alone without a reading, a row either way: a collapsed one would arrange to zero
    // height and read as "no bar". A reading adds one body line, 13 px at the theme's factor,
    // which the shaper measures up to a whole pixel: 15.59 to 16.
    let theme = Theme::default();
    let line = theme.palantir.text.line_height_for(theme.text.body).ceil();
    assert_eq!(without.size.h, 2.0 * PAD_Y, "no reading");
    assert_eq!(with.size.h, 2.0 * PAD_Y + line, "a 3 MiB reading");
    for (rect, what) in [(without, "no reading"), (with, "3 MiB reading")] {
        assert_eq!(rect.size.w, 1200.0, "{what}: the bar spans the window");
    }
}
