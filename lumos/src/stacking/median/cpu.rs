//! CPU dispatch for median stacking.

/// Calculate median of values.
#[inline]
pub(super) fn median_f32(values: &[f32]) -> f32 {
    super::scalar::median_f32(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_median_odd() {
        let values = vec![5.0, 1.0, 3.0, 2.0, 4.0];
        let result = median_f32(&values);
        assert!((result - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_even() {
        let values = vec![4.0, 1.0, 3.0, 2.0];
        let result = median_f32(&values);
        assert!((result - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_single() {
        let values = vec![42.0];
        let result = median_f32(&values);
        assert!((result - 42.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_two() {
        let values = vec![1.0, 3.0];
        let result = median_f32(&values);
        assert!((result - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_large() {
        let values: Vec<f32> = (1..=100).map(|x| x as f32).rev().collect();
        let result = median_f32(&values);
        assert!((result - 50.5).abs() < f32::EPSILON);
    }
}
