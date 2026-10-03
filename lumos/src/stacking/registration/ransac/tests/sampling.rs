use super::*;

#[test]
fn random_sample_into_produces_unique_indices() {
    use rand::SeedableRng;
    let mut rng = SmallRng::seed_from_u64(42);
    let n = 50;
    let k = 4;
    let mut buffer = Vec::new();
    let mut indices = Vec::new();

    for _ in 0..200 {
        random_sample_into(&mut rng, n, k, &mut buffer, &mut indices);

        assert_eq!(buffer.len(), k);
        // All indices in range
        for &idx in &buffer {
            assert!(idx < n, "Index {idx} out of range 0..{n}");
        }
        // All indices unique
        let mut sorted = buffer.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), k, "Duplicate indices: {buffer:?}");
        // Persistent array stays valid (undone swaps)
        assert_eq!(indices.len(), n);
        for (i, &v) in indices.iter().enumerate() {
            assert_eq!(v, i, "indices[{i}] = {v}, expected {i}");
        }
    }
}

#[test]
fn random_sample_into_k_equals_n() {
    // When k == n, should return all indices (in some order)
    use rand::SeedableRng;
    let mut rng = SmallRng::seed_from_u64(99);
    let n = 5;
    let k = 5;
    let mut buffer = Vec::new();
    let mut indices = Vec::new();

    random_sample_into(&mut rng, n, k, &mut buffer, &mut indices);

    assert_eq!(buffer.len(), 5);
    let mut sorted = buffer.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
}

#[test]
fn weighted_sample_into_pool_smaller_than_k() {
    // When pool.len() <= k, should return all pool elements
    use rand::SeedableRng;
    let mut rng = SmallRng::seed_from_u64(42);
    let pool = vec![5, 10, 15];
    let weights = vec![0.0; 20]; // weights indexed by pool values
    let mut buffer = Vec::new();
    let mut scratch = Vec::new();

    weighted_sample_into(&mut rng, &pool, &weights, 5, &mut buffer, &mut scratch);
    let mut sorted = buffer.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, vec![5, 10, 15]);
}

/// Weighted sampling draws each index with probability proportional to its weight: for one draw,
/// A-Res picks the largest key `uᵢ^(1/wᵢ)`, which is index `i` with probability `wᵢ/Σw`. Weights
/// `i + 1` over 20 indices sum to 210, so in 21 000 draws index `i` is expected `100·(i + 1)`
/// times, with binomial standard deviation `√(N·p·(1 − p))`; every count lands within five of them.
/// Uniform sampling would give every index 1050 and miss by far more. Draws of four are distinct
/// and from the pool.
#[test]
fn weighted_sampling_follows_the_weights() {
    use rand::SeedableRng;
    let mut rng = SmallRng::seed_from_u64(42);
    let pool: Vec<usize> = (0..20).collect();
    let weights: Vec<f64> = (0..20).map(|i| f64::from(i) + 1.0).collect();
    let mut buffer = Vec::new();
    let mut scratch = Vec::new();

    const DRAWS: usize = 21_000;
    let mut counts = [0usize; 20];
    for _ in 0..DRAWS {
        weighted_sample_into(&mut rng, &pool, &weights, 1, &mut buffer, &mut scratch);
        counts[buffer[0]] += 1;
    }
    for (i, &count) in counts.iter().enumerate() {
        let p = (i as f64 + 1.0) / 210.0;
        let expected = DRAWS as f64 * p;
        let sigma = (DRAWS as f64 * p * (1.0 - p)).sqrt();
        assert!(
            (count as f64 - expected).abs() <= 5.0 * sigma,
            "index {i}: {count} draws, expected {expected} ± {sigma}"
        );
    }

    for _ in 0..100 {
        weighted_sample_into(&mut rng, &pool, &weights, 4, &mut buffer, &mut scratch);
        let mut sorted = buffer.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "duplicates in {buffer:?}");
        assert!(buffer.iter().all(|&idx| idx < 20));
    }
}
