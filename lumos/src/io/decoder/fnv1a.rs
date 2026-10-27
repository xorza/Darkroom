//! FNV-1a 64, the hash every name and version the decode cache derives is made with.

/// FNV-1a 64's starting state.
pub(crate) const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64 continued from `state` over `bytes`.
pub(crate) const fn hash(mut state: u64, bytes: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut byte = 0;
    while byte < bytes.len() {
        state = (state ^ bytes[byte] as u64).wrapping_mul(PRIME);
        byte += 1;
    }
    state
}

/// FNV-1a over `pins`, each followed by a `0xff`, which no UTF-8 text holds, so no two lists of
/// text hash as one concatenation. A version derived from pinned text rather than a number bumped
/// by hand.
pub(crate) const fn fingerprint(pins: &[&[u8]]) -> u64 {
    let mut state = OFFSET;
    let mut pin = 0;
    while pin < pins.len() {
        state = hash(hash(state, pins[pin]), &[0xff]);
        pin += 1;
    }
    state
}
