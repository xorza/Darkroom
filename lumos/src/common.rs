//! Common utilities for lumos.

use rayon::prelude::*;

/// CPU feature flags detected once at startup.
#[cfg(target_arch = "x86_64")]
pub mod cpu_features {
    use std::sync::OnceLock;

    #[derive(Debug, Clone, Copy)]
    pub struct X86Features {
        pub sse2: bool,
        pub sse3: bool,
        pub sse4_1: bool,
    }

    static FEATURES: OnceLock<X86Features> = OnceLock::new();

    /// Get cached CPU features (detected once on first call).
    #[inline]
    pub fn get() -> X86Features {
        *FEATURES.get_or_init(|| X86Features {
            sse2: is_x86_feature_detected!("sse2"),
            sse3: is_x86_feature_detected!("sse3"),
            sse4_1: is_x86_feature_detected!("sse4.1"),
        })
    }

    /// Check if SSE2 is available.
    #[inline]
    pub fn has_sse2() -> bool {
        get().sse2
    }

    /// Check if SSE3 is available.
    #[inline]
    pub fn has_sse3() -> bool {
        get().sse3
    }

    /// Check if SSE4.1 is available.
    #[inline]
    pub fn has_sse4_1() -> bool {
        get().sse4_1
    }
}

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
}
