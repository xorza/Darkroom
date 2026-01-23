//! Common utilities for lumos.

use rayon::prelude::*;

/// Chunk size for parallel operations to avoid false cache sharing.
/// 16KB worth of f32 values (4096 elements) ensures each chunk spans
/// multiple cache lines and reduces false sharing at chunk boundaries.
const CHUNK_SIZE: usize = 4096;

/// Apply a function to each index in parallel, collecting results into a Vec<f32>.
/// Uses explicit chunking to avoid false cache sharing between threads.
pub fn parallel_map_f32<F>(len: usize, f: F) -> Vec<f32>
where
    F: Fn(usize) -> f32 + Sync + Send,
{
    if len == 0 {
        return Vec::new();
    }

    let mut result = vec![0.0f32; len];

    // Process chunks in parallel, each thread writes to its own chunk
    result
        .par_chunks_mut(CHUNK_SIZE)
        .enumerate()
        .for_each(|(chunk_idx, chunk)| {
            let start_idx = chunk_idx * CHUNK_SIZE;
            for (i, val) in chunk.iter_mut().enumerate() {
                *val = f(start_idx + i);
            }
        });

    // Trim to exact length if last chunk was partial
    result.truncate(len);
    debug_assert_eq!(result.len(), len);

    result
}

/// Compute sum of f32 slice in parallel using rayon.
/// Each thread computes a partial sum, then results are combined.
pub fn parallel_sum_f32(values: &[f32]) -> f32 {
    values
        .par_chunks(CHUNK_SIZE)
        .map(|chunk| chunk.iter().sum::<f32>())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_map_f32() {
        let result = parallel_map_f32(10, |i| i as f32 * 2.0);
        assert_eq!(result.len(), 10);
        for (i, &v) in result.iter().enumerate() {
            assert!((v - i as f32 * 2.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn test_parallel_sum_f32() {
        let values: Vec<f32> = (1..=100).map(|x| x as f32).collect();
        let result = parallel_sum_f32(&values);
        let expected: f32 = values.iter().sum();
        assert!((result - expected).abs() < f32::EPSILON);
    }
}
