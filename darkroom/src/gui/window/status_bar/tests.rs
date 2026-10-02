use super::*;
use crate::core::document::harness::DocFixture;
use crate::gui::app::session::harness::SessionHarness;

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

/// The bar used to collapse when it had nothing to say; the process
/// footprint gives it something on every frame, so it is recorded on
/// an untouched document — and stays recorded when no reading is
/// available, rather than reappearing as the figure lands.
#[test]
fn status_bar_is_recorded_on_an_idle_document_with_or_without_a_reading() {
    let mut h = SessionHarness::new(DocFixture::default());
    h.prime(2);
    let without =
        h.ui.rect(status_bar_id())
            .expect("status bar records with no reading and an empty cache");

    h.process_memory = 3 * 1024 * 1024;
    h.prime(2);
    let with = h.ui.rect(status_bar_id()).expect("status bar records");

    // The strip is a real row either way — a collapsed one would
    // arrange to zero height and read as "no bar".
    for (rect, what) in [(without, "no reading"), (with, "3 MB reading")] {
        assert!(rect.size.h > 0.0, "{what}: bar arranged to zero height");
        assert!(rect.size.w > 0.0, "{what}: bar arranged to zero width");
    }
    // With a reading the bar hugs a line of text; without one it is
    // padding alone, so it is strictly shorter. Both are still rows.
    assert!(
        with.size.h > without.size.h,
        "a reading adds its label's line to the bar: {} vs {}",
        with.size.h,
        without.size.h,
    );
}
