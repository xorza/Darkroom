use crate::stacking::star_detection::labeling::tests::*;

/// Simple PRNG for deterministic tests
fn simple_rng(seed: u64, index: usize) -> bool {
    let mut x = seed.wrapping_add(index as u64);
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545F4914F6CDD1D) & 1 == 0
}

#[test]
fn random_sparse_masks() {
    // Test random sparse masks (typical for star detection)
    for seed in 0..10 {
        let width = 50 + seed * 7;
        let height = 50 + seed * 5;
        let mask: Vec<bool> = (0..width * height)
            .map(|i| simple_rng(seed as u64 * 1000, i) && simple_rng(seed as u64 * 1001, i))
            .collect();

        compare_with_reference(&mask, width, height);
    }
}

#[test]
fn random_dense_masks() {
    // Test random dense masks
    for seed in 0..5 {
        let width = 30 + seed * 3;
        let height = 30 + seed * 4;
        let mask: Vec<bool> = (0..width * height)
            .map(|i| simple_rng(seed as u64 * 2000, i))
            .collect();

        compare_with_reference(&mask, width, height);
    }
}

#[test]
fn random_large_masks() {
    // Test larger masks that use parallel algorithm
    for seed in 0..3 {
        let width = 400;
        let height = 300;
        // Sparse: ~5% density
        let mask: Vec<bool> = (0..width * height)
            .map(|i| {
                simple_rng(seed as u64 * 3000, i)
                    && simple_rng(seed as u64 * 3001, i)
                    && simple_rng(seed as u64 * 3002, i)
                    && simple_rng(seed as u64 * 3003, i)
            })
            .collect();

        compare_with_reference(&mask, width, height);
    }
}

#[test]
fn invariants_on_random_masks() {
    // Verify invariants hold on various random masks
    for seed in 0..20 {
        let width = 20 + (seed % 30);
        let height = 20 + (seed % 25);
        let density = (seed % 5) as f64 * 0.15 + 0.05; // 5% to 65%

        let mask: Vec<bool> = (0..width * height)
            .map(|i| {
                let hash = simple_rng(seed as u64 * 5000, i);
                let threshold = (density * u64::MAX as f64) as u64;
                let val = (seed as u64 * 5000)
                    .wrapping_add(i as u64)
                    .wrapping_mul(0x2545F4914F6CDD1D);
                hash && val < threshold
            })
            .collect();

        let bit_mask = BitBuffer2::from_slice(width, height, &mask);

        let label_map_4 = label_map_from_mask_with_connectivity(&bit_mask, Connectivity::Four);
        verify_ccl_invariants(
            &mask,
            label_map_4.labels(),
            width,
            height,
            Connectivity::Four,
        );

        let label_map_8 = label_map_from_mask_with_connectivity(&bit_mask, Connectivity::Eight);
        verify_ccl_invariants(
            &mask,
            label_map_8.labels(),
            width,
            height,
            Connectivity::Eight,
        );
    }
}
