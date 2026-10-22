use crate::registration::ransac::ransac_config::RansacConfig;
use crate::registration::ransac::sampling::Sampler;

/// A sampler of `sample_size` over pairs at `confidences`, under seed 42.
fn sampler(confidences: &[f64], sample_size: usize) -> Sampler {
    Sampler::new(
        confidences,
        sample_size,
        &RansacConfig {
            seed: 42,
            ..Default::default()
        },
    )
}

/// Each uniform draw of 4 from 50 is 4 distinct indices in range, and the shuffle is `0..50` again
/// after it.
#[test]
fn uniform_draws_distinct_indices_and_restores_the_shuffle() {
    let n = 50;
    let mut sampler = sampler(&[1.0; 50], 4);
    let mut sample = Vec::new();
    for _ in 0..200 {
        sampler.uniform(&mut sample);
        assert_eq!(sample.len(), 4);
        assert!(sample.iter().all(|&idx| idx < n), "{sample:?}");
        let mut sorted = sample.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "duplicates in {sample:?}");
        assert_eq!(sampler.shuffle, (0..n).collect::<Vec<_>>());
    }
}

/// A uniform draw of every pair is a permutation of them.
#[test]
fn uniform_draw_of_every_pair_is_a_permutation() {
    let mut sampler = sampler(&[1.0; 5], 5);
    let mut sample = Vec::new();
    sampler.uniform(&mut sample);
    sample.sort_unstable();
    assert_eq!(sample, [0, 1, 2, 3, 4]);
}

/// A pool no larger than a sample is taken whole: the three most confident of five pairs, in
/// confidence order, for a sample of four.
#[test]
fn weighted_draw_takes_a_small_pool_whole() {
    let mut sampler = sampler(&[0.2, 0.9, 0.1, 0.5, 0.7], 4);
    let mut sample = Vec::new();
    sampler.weighted(3, &mut sample);
    assert_eq!(sample, [1, 4, 3]);
}

/// Weighted sampling draws each index with probability proportional to its weight: for one draw,
/// A-Res picks the largest key `uᵢ^(1/wᵢ)`, which is index `i` with probability `wᵢ/Σw`. A
/// confidence of `√(i + 1) − 0.1` weighs pair `i` at `(c + 0.1)² = i + 1`, to rounding; over 20
/// pairs those sum to 210, so in 21 000 draws index `i` is expected `100·(i + 1)` times, with
/// binomial standard deviation `√(N·p·(1 − p))`, and every count lands within five of them.
/// Uniform sampling would give every index 1050 and miss by far more. Draws of four are distinct
/// and from the pool.
#[test]
fn weighted_sampling_follows_the_weights() {
    const DRAWS: usize = 21_000;

    let confidences: Vec<f64> = (0..20).map(|i| (f64::from(i) + 1.0).sqrt() - 0.1).collect();
    let mut single = sampler(&confidences, 1);
    let mut sample = Vec::new();
    let mut counts = [0usize; 20];
    for _ in 0..DRAWS {
        single.weighted(20, &mut sample);
        counts[sample[0]] += 1;
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

    let mut four = sampler(&confidences, 4);
    for _ in 0..100 {
        four.weighted(20, &mut sample);
        let mut sorted = sample.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "duplicates in {sample:?}");
        assert!(sample.iter().all(|&idx| idx < 20));
    }
}

/// The guided phases draw from the most confident pairs, then the uniform phase from all. Twenty
/// pairs, confident in reverse index order, give pools of the top 5 (indices 15–19) and the top 10
/// (10–19); a sample of 2 at confidence 0.995 has phases of
/// `⌈ln 0.005 / ln (1 − 0.5²)⌉ = ⌈18.42⌉ = 19` iterations.
#[test]
fn guided_phases_draw_from_the_most_confident_pools() {
    let confidences: Vec<f64> = (0..20).map(f64::from).collect();
    let mut sampler = sampler(&confidences, 2);
    assert_eq!(sampler.guided_iterations(), 38);
    let mut sample = Vec::new();
    let mut seen_below_ten = false;
    for iteration in 1..=200 {
        sampler.draw(iteration, &mut sample);
        assert_eq!(sample.len(), 2);
        let floor = match iteration {
            1..=19 => 15,
            20..=38 => 10,
            _ => 0,
        };
        assert!(
            sample.iter().all(|&idx| idx >= floor),
            "iteration {iteration}: {sample:?}"
        );
        seen_below_ten |= sample.iter().any(|&idx| idx < 10);
    }
    assert!(seen_below_ten, "the uniform phase reaches every pair");
}
