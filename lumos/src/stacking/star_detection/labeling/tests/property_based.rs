use crate::math::size2us::Size2us;
use std::ops::Range;

use crate::stacking::star_detection::labeling::tests::*;
use crate::testing::TestRng;

#[derive(Debug)]
struct RandomMaskCase {
    size: Size2us,
    density: f64,
    seeds: Range<u64>,
}

fn random_mask(case: &RandomMaskCase, seed: u64) -> Vec<bool> {
    let mut rng = TestRng::new(seed);
    (0..case.size.width * case.size.height)
        .map(|_| rng.next_f64() < case.density)
        .collect()
}

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
            size: Size2us::new(400, 300),
            density: 0.05,
            seeds: 25..28,
        },
    ];

    for case in cases {
        for seed in case.seeds.clone() {
            let mask = random_mask(&case, seed);
            compare_with_reference(&mask, case.size);
        }
    }
}
