//! Scalar (non-SIMD) implementation of mean stacking operations.

/// Accumulate src into dst using scalar operations.
#[inline]
#[cfg_attr(all(target_arch = "aarch64", not(feature = "bench")), allow(dead_code))]
pub(super) fn accumulate_chunk(dst: &mut [f32], src: &[f32]) {
    for (d, &s) in dst.iter_mut().zip(src.iter()) {
        *d += s;
    }
}

/// Divide all values by a scalar using scalar operations.
#[inline]
#[cfg_attr(all(target_arch = "aarch64", not(feature = "bench")), allow(dead_code))]
pub(super) fn divide_chunk(data: &mut [f32], inv_count: f32) {
    for d in data.iter_mut() {
        *d *= inv_count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accumulate_chunk() {
        let mut dst = vec![1.0, 2.0, 3.0, 4.0];
        let src = vec![0.5, 0.5, 0.5, 0.5];
        accumulate_chunk(&mut dst, &src);
        assert_eq!(dst, vec![1.5, 2.5, 3.5, 4.5]);
    }

    #[test]
    fn test_divide_chunk() {
        let mut data = vec![2.0, 4.0, 6.0, 8.0];
        divide_chunk(&mut data, 0.5);
        assert_eq!(data, vec![1.0, 2.0, 3.0, 4.0]);
    }
}
