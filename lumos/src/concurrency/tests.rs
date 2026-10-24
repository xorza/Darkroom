use crate::concurrency;

/// Zeros at every length: none, fewer than one block, and a block and a half, whose last piece is
/// short.
#[test]
fn zeroed_in_parallel_is_all_zeros_at_its_length() {
    for len in [0, 3, 3 * 1024 * 1024 / 4] {
        let plane = concurrency::zeroed_in_parallel(len);
        assert_eq!(plane.len(), len);
        assert!(plane.iter().all(|&value| value.to_bits() == 0));
    }
}
