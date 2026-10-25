//! [`ImageData`]: what LibRaw identifies of a file's sensor and camera.

use std::ffi;

/// What LibRaw identifies of a file's sensor and camera at open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageData {
    /// The visible area's `filters` word: the color of each position of an 8 × 2 block, `0` for
    /// no mosaic, or [`XTRANS_FILTERS`](crate::XTRANS_FILTERS) for an X-Trans one.
    pub filters: u32,
    /// The colors the sensor records: 1 for a monochrome one, 3 for RGB, 4 for RGBE or CMYG.
    pub colors: i32,
    /// The colors LibRaw's `cdesc` names, one letter each.
    pub cdesc: [u8; 4],
    /// The 6 × 6 X-Trans layout, a color index each, where `filters` names one.
    pub xtrans: [[u8; 6]; 6],
    /// The camera's maker, as LibRaw normalizes it; empty where it names none.
    pub make: String,
    /// The camera's model, as LibRaw normalizes it; empty where it names none.
    pub model: String,
    /// The file's DNG version, `0` for a file that is no DNG.
    pub dng_version: u32,
}

/// The byte value of each `c_char`: `c_char` is `i8` on some targets and `u8` on others.
pub(crate) fn bytes<const N: usize>(chars: [ffi::c_char; N]) -> [u8; N] {
    chars.map(|char| u8::from_ne_bytes(char.to_ne_bytes()))
}

/// A NUL-terminated field, read up to its terminator.
pub(crate) fn text<const N: usize>(chars: [ffi::c_char; N]) -> String {
    let bytes = bytes(chars);
    let len = bytes.iter().position(|&byte| byte == 0).unwrap_or(N);
    String::from_utf8_lossy(&bytes[..len]).into_owned()
}

#[cfg(test)]
mod tests {
    use std::ffi;

    use crate::image_data::text;

    /// A field reads up to its terminator, whatever follows it, and whole where it has none.
    #[test]
    fn a_field_reads_to_its_terminator() {
        let field = |bytes: &[u8]| {
            let mut field: [ffi::c_char; 8] = [0; 8];
            for (slot, &byte) in field.iter_mut().zip(bytes) {
                *slot = ffi::c_char::from_ne_bytes([byte]);
            }
            field
        };
        assert_eq!(text(field(b"Canon\0xy")), "Canon");
        assert_eq!(text(field(b"")), "");
        assert_eq!(text(field(b"ABCDEFGH")), "ABCDEFGH");
    }
}
