use std::ops::Range;

use crate::stacking::star_detection::labeling::tests::*;
use crate::testing::TestRng;

#[derive(Debug)]
struct RandomMaskCase {
    width: usize,
    height: usize,
    density: f64,
    seeds: Range<u64>,
}

fn random_mask(case: &RandomMaskCase, seed: u64) -> Vec<bool> {
    let mut rng = TestRng::new(seed);
    (0..case.width * case.height)
        .map(|_| rng.next_f64() < case.density)
        .collect()
}

#[test]
fn random_masks_match_reference() {
    let cases = [
        RandomMaskCase {
            width: 64,
            height: 60,
            density: 0.25,
            seeds: 0..10,
        },
        RandomMaskCase {
            width: 42,
            height: 46,
            density: 0.5,
            seeds: 10..15,
        },
        RandomMaskCase {
            width: 50,
            height: 45,
            density: 0.05,
            seeds: 15..20,
        },
        RandomMaskCase {
            width: 50,
            height: 45,
            density: 0.65,
            seeds: 20..25,
        },
        RandomMaskCase {
            width: 400,
            height: 300,
            density: 0.05,
            seeds: 25..28,
        },
    ];

    for case in cases {
        for seed in case.seeds.clone() {
            let mask = random_mask(&case, seed);
            compare_with_reference(&mask, case.width, case.height);
        }
    }
}
