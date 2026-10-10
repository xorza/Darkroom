use libraw_sys as sys;

/// The shim reads the flag `libraw_open_bayer` sets from `procflags & 2`, and frees nothing it
/// does not own. A plain Bayer dump is no SuperCCD, no float file and no CR3, and states no
/// capture time, and the accessors say so.
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
            assert_eq!(sys::libraw_lumos_fuji_width(raw), 0);
            assert_eq!(sys::libraw_lumos_is_floating_point(raw), 0);
            let (mut enc_type, mut image_levels) = (-1, -1);
            assert_eq!(
                sys::libraw_lumos_crx_coding(raw, &mut enc_type, &mut image_levels),
                0
            );
            assert_eq!((enc_type, image_levels), (-1, -1), "nothing written");
            let mut clock = [0 as core::ffi::c_char; 20];
            assert_eq!(
                sys::libraw_lumos_camera_clock(raw, clock.as_mut_ptr(), clock.len()),
                0,
                "a sensor dump states no time"
            );
            sys::libraw_close(raw);
        }
    }
}

/// The library carries OpenMP exactly when the build script found a runtime that links, which it
/// states as `cfg(libraw_openmp)`; setting the decode threads is harmless either way.
#[test]
fn the_build_carries_openmp_where_the_toolchain_links_it() {
    // SAFETY: both calls touch only the calling thread's OpenMP state.
    unsafe {
        sys::libraw_lumos_set_decode_threads(2);
        assert_eq!(sys::libraw_lumos_openmp() != 0, cfg!(libraw_openmp));
    }
}
