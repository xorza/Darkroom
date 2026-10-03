use std::ops::Range;

use crate::stacking::star_detection::labeling::tests::{Mask, check};
use crate::testing::prelude::*;

#[derive(Debug)]
struct RandomMaskCase {
    size: Size2us,
    density: f64,
    seeds: Range<u64>,
}

fn random_mask(case: &RandomMaskCase, seed: u64) -> Mask {
    let mut rng = TestRng::new(seed);
    Mask::from_fn(case.size, |_, _| rng.next_f64() < case.density)
}

/// Random masks across densities and sizes, each against the reference at every band count.
#[test]
fn random_masks_match_reference() {
    let cases = [
        RandomMaskCase {
            size: Size2us::new(64, 60),
            density: 0.25,
            seeds: 0..10,
        },
        RandomMaskCase {
            size: Size2us::new(42, 46),
            density: 0.5,
            seeds: 10..15,
        },
        RandomMaskCase {
            size: Size2us::new(50, 45),
            density: 0.05,
            seeds: 15..20,
        },
        RandomMaskCase {
            size: Size2us::new(50, 45),
            density: 0.65,
            seeds: 20..25,
        },
        RandomMaskCase {
            size: Size2us::new(150, 300),
            density: 0.3,
            seeds: 25..28,
        },
        RandomMaskCase {
            size: Size2us::new(400, 300),
            density: 0.05,
            seeds: 28..31,
        },
    ];

    for case in cases {
        for seed in case.seeds.clone() {
            check(&random_mask(&case, seed));
        }
    }
}
