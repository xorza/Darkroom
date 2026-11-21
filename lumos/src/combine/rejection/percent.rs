//! A share of a sample count, exact.

/// `⌊percent·count/100⌋`, exact for `percent` in `[0, 100]`. `percent·count` is exact in f64, a
/// 24-bit significand times an integer below 2²⁹, and so are the multiples of 100 near it; only
/// the quotient rounds, so the multiples settle its floor. `(p/100)·n` in f32 gives 62 for 42% of
/// 150.
#[expect(
    clippy::cast_sign_loss,
    reason = "the callers' validation holds `percent` in [0, 100]"
)]
pub(super) fn share_of(percent: f32, count: usize) -> usize {
    debug_assert!((0.0..=100.0).contains(&percent));
    debug_assert!(count < 1 << 29);
    let product = f64::from(percent) * count as f64;
    let mut share = (product / 100.0).floor();
    if share * 100.0 > product {
        share -= 1.0;
    } else if (share + 1.0) * 100.0 <= product {
        share += 1.0;
    }
    share as usize
}
