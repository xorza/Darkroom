use crate::internals::synthetic::fixtures::*;
use crate::internals::synthetic::metrics::pixel_stats;
use imaginarium::Buffer2;

fn region_sum(px: &Buffer2<f32>, x0: usize, y0: usize, size: usize) -> f64 {
    let mut s = 0.0;
    for y in y0..y0 + size {
        for x in x0..x0 + size {
            s += f64::from(px[(x, y)]);
        }
    }
    s
}

#[test]
fn star_field_has_requested_sources_and_signal() {
    let frame = star_field(Size2us::new(128, 128), 30, 1);
    assert_eq!(frame.truth.sources.len(), 30);
    assert_eq!(frame.image.channel(0).pixels().len(), 128 * 128);
    // Bright stars on a 0.1 sky: mean above background, a clear peak.
    let s = pixel_stats(frame.image.channel(0).pixels());
    assert!(s.mean > 0.1, "mean {}", s.mean);
    let peak = frame
        .image
        .channel(0)
        .pixels()
        .iter()
        .copied()
        .fold(0.0f32, f32::max);
    assert!(peak > 0.3, "peak {peak}");
    assert!(peak < 0.95, "the brightest star saturates: {peak}");
}

#[test]
fn cluster_field_is_denser_at_center() {
    let frame = cluster_field(Size2us::new(200, 200), 400, 2);
    assert_eq!(frame.truth.sources.len(), 400);
    let px = frame.image.channel(0);
    // Central 40×40 carries much more flux than a corner 40×40.
    let center = region_sum(px, 80, 80, 40);
    let corner = region_sum(px, 2, 2, 40);
    assert!(center > corner * 3.0, "center {center} corner {corner}");
}
