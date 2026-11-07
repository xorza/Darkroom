//! The main thread stack the binary states, which no other test runs on.

use std::io::Read;

use common::fs;

/// What `build.rs` reserves.
const RESERVE: u64 = 8 * 1024 * 1024;

/// Past a PE image's headers up to its stack reserve.
const HEADER_LEN: usize = 1024;

/// A PE image's `SizeOfStackReserve`, as the PE format lays it out: the offset of the signature at
/// 0x3C, then the 4-byte signature and the 20-byte file header, then the optional header, which
/// holds the reserve at 72. `None` for another system's binary, whose format states no reserve.
fn stack_reserve(header: &[u8]) -> Option<u64> {
    if !header.starts_with(b"MZ") {
        return None;
    }
    let signature = u32::from_le_bytes(header[0x3C..0x40].try_into().unwrap());
    let signature = usize::try_from(signature).unwrap();
    assert_eq!(&header[signature..signature + 4], b"PE\0\0");
    let optional = signature + 4 + 20;
    assert_eq!(
        &header[optional..optional + 2],
        &0x20B_u16.to_le_bytes(),
        "a 64-bit image"
    );
    let reserve = optional + 72;
    Some(u64::from_le_bytes(
        header[reserve..reserve + 8].try_into().unwrap(),
    ))
}

/// A header built by hand, with the signature at 0x80: the optional header is at
/// 0x80 + 4 + 20 = 0x98, and the reserve at 0x98 + 72 = 0xE0.
#[test]
fn the_reserve_is_read_from_a_pe_header() {
    let mut header = vec![0_u8; HEADER_LEN];
    header[..2].copy_from_slice(b"MZ");
    header[0x3C] = 0x80;
    header[0x80..0x84].copy_from_slice(b"PE\0\0");
    header[0x98..0x9A].copy_from_slice(&0x20B_u16.to_le_bytes());
    header[0xE0..0xE8].copy_from_slice(&123_456_u64.to_le_bytes());
    assert_eq!(stack_reserve(&header), Some(123_456));

    assert_eq!(stack_reserve(b"\x7FELF"), None, "not a PE image");
}

/// On Windows the binary states the reserve; elsewhere its format states none, and the system's
/// limit applies.
#[test]
fn the_binary_reserves_its_main_stack() {
    let mut header = [0_u8; HEADER_LEN];
    fs::open(env!("CARGO_BIN_EXE_darkroom"))
        .unwrap()
        .read_exact(&mut header)
        .unwrap();
    if let Some(reserve) = stack_reserve(&header) {
        assert_eq!(reserve, RESERVE);
    }
}
