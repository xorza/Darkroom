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

#[test]
fn weighted_sample_into_returns_k_unique() {
    use rand::SeedableRng;
    let mut rng = SmallRng::seed_from_u64(42);
    let pool: Vec<usize> = (0..20).collect();
    let weights: Vec<f64> = (0..20).map(|i| f64::from(i) + 1.0).collect();
    let k = 4;
    let mut buffer = Vec::new();
    let mut scratch = Vec::new();

    for _ in 0..100 {
        weighted_sample_into(&mut rng, &pool, &weights, k, &mut buffer, &mut scratch);
        assert_eq!(buffer.len(), k);

        // All unique
        let mut sorted = buffer.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), k, "Duplicates in weighted sample: {buffer:?}");

        // All from pool
        for &idx in &buffer {
            assert!(idx < 20);
        }
    }
}
