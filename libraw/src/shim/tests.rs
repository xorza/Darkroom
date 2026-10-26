use super::*;
use crate::sys;

/// The static library links, and both it and the headers it was bound from are the pinned tag.
#[test]
fn linked_library_matches_bindings() {
    // LIBRAW_MAKE_VERSION: (major << 16) | (minor << 8) | patch.
    let expected = (sys::LIBRAW_MAJOR_VERSION << 16)
        | (sys::LIBRAW_MINOR_VERSION << 8)
        | sys::LIBRAW_PATCH_VERSION;
    let linked = unsafe { sys::libraw_versionNumber() };
    assert_eq!(u32::try_from(linked).unwrap(), expected);
    assert_eq!(expected, 0x00_16_02, "0.22.2");
}

/// A plain Bayer dump is no SuperCCD, no float file and no CR3, and states no capture time: the
/// shims say so, write nothing past what they report, and free nothing they do not own.
#[test]
fn the_shims_read_a_sensor_dump_as_plain() {
    const SIDE: u16 = 24;
    let mut bytes = vec![0x10u8; usize::from(SIDE) * usize::from(SIDE) * 2];
    unsafe {
        let raw = sys::libraw_init(0);
        assert!(!raw.is_null());
        let opened = sys::libraw_open_bayer(
            raw,
            bytes.as_mut_ptr(),
            u32::try_from(bytes.len()).unwrap(),
            SIDE,
            SIDE,
            0,
            0,
            0,
            0,
            0,
            0x94,
            0,
            0,
            0,
        );
        assert_eq!(opened, 0);
        assert_eq!(libraw_lumos_fuji_width(raw), 0);
        assert_eq!(libraw_lumos_is_floating_point(raw), 0);
        let (mut enc_type, mut image_levels) = (-1, -1);
        assert_eq!(
            libraw_lumos_crx_coding(raw, &raw mut enc_type, &raw mut image_levels),
            0
        );
        assert_eq!((enc_type, image_levels), (-1, -1), "nothing written");
        let mut clock: [core::ffi::c_char; 20] = [0; 20];
        assert_eq!(
            libraw_lumos_camera_clock(raw, clock.as_mut_ptr(), clock.len()),
            0,
            "a sensor dump states no time"
        );
        sys::libraw_close(raw);
    }
}

/// The library carries OpenMP exactly when the build script found a runtime that links, which it
/// states as `cfg(libraw_openmp)`; setting the decode threads is harmless either way.
#[test]
fn the_build_carries_openmp_where_the_toolchain_links_it() {
    unsafe {
        libraw_lumos_set_decode_threads(2);
        assert_eq!(libraw_lumos_openmp() != 0, cfg!(libraw_openmp));
    }
}
