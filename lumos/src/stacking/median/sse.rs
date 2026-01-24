//! SSE SIMD optimizations for median calculation (x86_64).
//!
//! Uses SIMD min/max operations for efficient sorting network-based median.

use std::arch::x86_64::*;

/// Calculate median using SSE SIMD sorting network for small arrays.
///
/// # Safety
/// Caller must ensure SSE2 is available.
#[target_feature(enable = "sse2")]
pub(super) unsafe fn median_f32(values: &mut [f32]) -> f32 {
    let len = values.len();
    let v = values.as_mut_ptr();

    unsafe {
        match len {
            0 => panic!("Cannot compute median of empty slice"),
            1 => *v,
            2 => (*v + *v.add(1)) * 0.5,
            3 => median_3(v),
            4 => median_4(v),
            5 => median_5(v),
            6 => median_6(v),
            7 => median_7(v),
            8 => median_8(v),
            9..=16 => median_small(values),
            _ => median_large(values),
        }
    }
}

/// Fall back to quickselect for larger arrays.
#[inline]
unsafe fn median_large(values: &mut [f32]) -> f32 {
    let len = values.len();
    let mid = len / 2;
    if len.is_multiple_of(2) {
        let (_, right, _) = values.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
        let right_val = *right;
        let left_val = values[..mid]
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        (left_val + right_val) * 0.5
    } else {
        let (_, median, _) = values.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
        *median
    }
}

/// SIMD min/max swap using pointers.
#[inline(always)]
unsafe fn minmax(a: *mut f32, b: *mut f32) {
    unsafe {
        let va = _mm_load_ss(a);
        let vb = _mm_load_ss(b);
        let min = _mm_min_ss(va, vb);
        let max = _mm_max_ss(va, vb);
        *a = _mm_cvtss_f32(min);
        *b = _mm_cvtss_f32(max);
    }
}

/// Median of 3 elements.
#[inline(always)]
unsafe fn median_3(v: *mut f32) -> f32 {
    unsafe {
        minmax(v, v.add(1));
        minmax(v.add(1), v.add(2));
        minmax(v, v.add(1));
        *v.add(1)
    }
}

/// Median of 4 elements (average of middle two).
#[inline(always)]
unsafe fn median_4(v: *mut f32) -> f32 {
    unsafe {
        minmax(v, v.add(1));
        minmax(v.add(2), v.add(3));
        minmax(v, v.add(2));
        minmax(v.add(1), v.add(3));
        minmax(v.add(1), v.add(2));
        (*v.add(1) + *v.add(2)) * 0.5
    }
}

/// Median of 5 elements.
#[inline(always)]
unsafe fn median_5(v: *mut f32) -> f32 {
    unsafe {
        minmax(v, v.add(1));
        minmax(v.add(3), v.add(4));
        minmax(v.add(2), v.add(4));
        minmax(v.add(2), v.add(3));
        minmax(v, v.add(3));
        minmax(v, v.add(2));
        minmax(v.add(1), v.add(4));
        minmax(v.add(1), v.add(3));
        minmax(v.add(1), v.add(2));
        *v.add(2)
    }
}

/// Median of 6 elements.
#[inline(always)]
unsafe fn median_6(v: *mut f32) -> f32 {
    unsafe {
        minmax(v, v.add(1));
        minmax(v.add(2), v.add(3));
        minmax(v.add(4), v.add(5));
        minmax(v, v.add(2));
        minmax(v.add(1), v.add(4));
        minmax(v.add(3), v.add(5));
        minmax(v, v.add(1));
        minmax(v.add(2), v.add(3));
        minmax(v.add(4), v.add(5));
        minmax(v.add(1), v.add(2));
        minmax(v.add(3), v.add(4));
        minmax(v.add(2), v.add(3));
        (*v.add(2) + *v.add(3)) * 0.5
    }
}

/// Median of 7 elements.
#[inline(always)]
unsafe fn median_7(v: *mut f32) -> f32 {
    unsafe {
        minmax(v, v.add(1));
        minmax(v.add(2), v.add(3));
        minmax(v.add(4), v.add(5));
        minmax(v, v.add(2));
        minmax(v.add(1), v.add(4));
        minmax(v.add(3), v.add(6));
        minmax(v.add(4), v.add(6));
        minmax(v.add(2), v.add(6));
        minmax(v, v.add(1));
        minmax(v.add(2), v.add(5));
        minmax(v.add(3), v.add(4));
        minmax(v.add(1), v.add(3));
        minmax(v.add(2), v.add(4));
        minmax(v.add(5), v.add(6));
        minmax(v.add(1), v.add(2));
        minmax(v.add(3), v.add(4));
        *v.add(3)
    }
}

/// Median of 8 elements using sorting network.
#[inline(always)]
unsafe fn median_8(v: *mut f32) -> f32 {
    unsafe {
        // Layer 1
        minmax(v, v.add(1));
        minmax(v.add(2), v.add(3));
        minmax(v.add(4), v.add(5));
        minmax(v.add(6), v.add(7));

        // Layer 2
        minmax(v, v.add(2));
        minmax(v.add(1), v.add(3));
        minmax(v.add(4), v.add(6));
        minmax(v.add(5), v.add(7));

        // Layer 3
        minmax(v, v.add(4));
        minmax(v.add(1), v.add(5));
        minmax(v.add(2), v.add(6));
        minmax(v.add(3), v.add(7));

        // Layer 4
        minmax(v.add(2), v.add(4));
        minmax(v.add(3), v.add(5));

        // Layer 5
        minmax(v.add(1), v.add(2));
        minmax(v.add(3), v.add(4));
        minmax(v.add(5), v.add(6));

        (*v.add(3) + *v.add(4)) * 0.5
    }
}

/// Median for 9-16 elements using insertion sort.
#[inline]
unsafe fn median_small(values: &mut [f32]) -> f32 {
    let len = values.len();
    let mid = len / 2;
    let v = values.as_mut_ptr();

    unsafe {
        // Insertion sort with SIMD min/max
        for i in 1..len {
            for j in (1..=i).rev() {
                let a = v.add(j - 1);
                let b = v.add(j);
                if *a <= *b {
                    break;
                }
                minmax(a, b);
            }
        }

        if len.is_multiple_of(2) {
            (*v.add(mid - 1) + *v.add(mid)) * 0.5
        } else {
            *v.add(mid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_median_3() {
        if !is_x86_feature_detected!("sse2") {
            return;
        }
        let mut v = vec![3.0, 1.0, 2.0];
        let result = unsafe { median_f32(&mut v) };
        assert!((result - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_4() {
        if !is_x86_feature_detected!("sse2") {
            return;
        }
        let mut v = vec![4.0, 1.0, 3.0, 2.0];
        let result = unsafe { median_f32(&mut v) };
        assert!((result - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_5() {
        if !is_x86_feature_detected!("sse2") {
            return;
        }
        let mut v = vec![5.0, 1.0, 4.0, 2.0, 3.0];
        let result = unsafe { median_f32(&mut v) };
        assert!((result - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_8() {
        if !is_x86_feature_detected!("sse2") {
            return;
        }
        let mut v = vec![8.0, 1.0, 7.0, 2.0, 6.0, 3.0, 5.0, 4.0];
        let result = unsafe { median_f32(&mut v) };
        assert!((result - 4.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_10() {
        if !is_x86_feature_detected!("sse2") {
            return;
        }
        let mut v: Vec<f32> = (1..=10).map(|x| x as f32).rev().collect();
        let result = unsafe { median_f32(&mut v) };
        assert!((result - 5.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_large() {
        if !is_x86_feature_detected!("sse2") {
            return;
        }
        let mut v: Vec<f32> = (1..=20).map(|x| x as f32).rev().collect();
        let result = unsafe { median_f32(&mut v) };
        assert!((result - 10.5).abs() < f32::EPSILON);
    }
}
