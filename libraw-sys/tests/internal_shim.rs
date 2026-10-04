use libraw_sys as sys;

/// The shim reads the flag `libraw_open_bayer` sets from `procflags & 2`, and frees nothing it
/// does not own.
#[test]
fn zero_is_bad_follows_the_open_flags() {
    const SIDE: u16 = 24;
    for (procflags, expected) in [(0u8, false), (2, true)] {
        let mut bytes = vec![0x10u8; usize::from(SIDE) * usize::from(SIDE) * 2];
        // SAFETY: libraw_init returns a valid pointer or null, checked below; the buffer outlives
        // the handle, which is closed before the loop iteration ends.
        unsafe {
            let raw = sys::libraw_init(0);
            assert!(!raw.is_null());
            let opened = sys::libraw_open_bayer(
                raw,
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                SIDE,
                SIDE,
                0,
                0,
                0,
                0,
                procflags,
                0x94,
                0,
                0,
                0,
            );
            assert_eq!(opened, 0);
            assert_eq!(sys::libraw_lumos_zero_is_bad(raw) != 0, expected);
            sys::libraw_close(raw);
        }
    }
}
