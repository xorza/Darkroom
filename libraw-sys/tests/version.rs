use libraw_sys as sys;

/// The static library links, and both it and the headers it was bound from are the pinned tag.
#[test]
fn linked_library_matches_bindings() {
    // LIBRAW_MAKE_VERSION: (major << 16) | (minor << 8) | patch.
    let expected = (sys::LIBRAW_MAJOR_VERSION << 16)
        | (sys::LIBRAW_MINOR_VERSION << 8)
        | sys::LIBRAW_PATCH_VERSION;
    // SAFETY: a pure query of a compile-time constant.
    let linked = unsafe { sys::libraw_versionNumber() };
    assert_eq!(u32::try_from(linked).unwrap(), expected);
    assert_eq!(expected, 0x00_16_02, "0.22.2");
}
