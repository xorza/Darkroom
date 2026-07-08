use super::*;

#[test]
fn hex_is_64_lowercase_chars() {
    let mut digest = [0u8; 32];
    digest[0] = 0xab;
    digest[31] = 0x0f;
    let mut buf = [0u8; 64];
    let h = hex(&Digest(digest), &mut buf);
    assert_eq!(h.len(), 64);
    assert!(h.starts_with("ab"));
    assert!(h.ends_with("0f"));
    assert!(
        h.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
}
