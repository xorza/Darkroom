//! `#[serde(with = "common::hex_bytes")]` for a field of bytes, a digest or a byte string: a text
//! format holds it as lowercase hex digits, two a byte, as blake3, git and `Cargo.lock` write a
//! digest, so a person reads it and a diff shows it on one line; a binary format holds the field
//! as its own type does, so a binary layout does not move.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Write `bytes` as hex digits in a text format, as its own type does in a binary one.
pub fn serialize<T, S>(bytes: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: AsRef<[u8]> + Serialize + ?Sized,
    S: Serializer,
{
    if serializer.is_human_readable() {
        serializer.serialize_str(&encode(bytes.as_ref()))
    } else {
        bytes.serialize(serializer)
    }
}

/// Read what [`serialize`] wrote. Hex digits of either case are read; a count of bytes the field
/// cannot hold is refused.
pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: Deserialize<'de> + TryFrom<Vec<u8>>,
    D: Deserializer<'de>,
{
    if !deserializer.is_human_readable() {
        return T::deserialize(deserializer);
    }
    // Owned: a reader-backed deserializer has no input to lend a `&str` from.
    let text = String::deserialize(deserializer)?;
    let bytes = decode(&text).ok_or_else(|| {
        D::Error::custom(format!(
            "{text:?} is not hex: an even count of the digits 0-9 and a-f, of either case"
        ))
    })?;
    let len = bytes.len();
    T::try_from(bytes).map_err(|_| D::Error::invalid_length(len, &"the byte count the field holds"))
}

fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    text
}

fn decode(text: &str) -> Option<Vec<u8>> {
    let digit = |c: u8| u8::try_from(char::from(c).to_digit(16)?).ok();
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|&[high, low]| Some((digit(high)? << 4) | digit(low)?))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use crate::serde::serde_format::SerdeFormat;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Fields {
        #[serde(with = "crate::serde::hex_bytes")]
        digest: [u8; 4],
        #[serde(with = "crate::serde::hex_bytes")]
        string: Vec<u8>,
    }

    /// RON holds each field as lowercase hex, two digits a byte, the empty string for no bytes;
    /// bitcode holds them as the types do; both read back. Upper-case digits read too, while an
    /// odd count of digits, a letter past `f`, and a digest of the wrong length are refused.
    #[test]
    fn bytes_are_hex_in_text_and_themselves_in_binary() {
        let fields = Fields {
            digest: [0x00, 0x0f, 0xa5, 0xff],
            string: Vec::new(),
        };
        let ron = SerdeFormat::Ron.serialize(&fields).unwrap();
        assert_eq!(
            String::from_utf8(ron.clone()).unwrap(),
            "(\n    digest: \"000fa5ff\",\n    string: \"\",\n)"
        );
        assert_eq!(
            SerdeFormat::Ron.deserialize::<Fields>(&ron).unwrap(),
            fields
        );

        let bitcode = SerdeFormat::Bitcode.serialize(&fields).unwrap();
        assert_eq!(
            bitcode,
            SerdeFormat::Bitcode
                .serialize(&([0x00_u8, 0x0f, 0xa5, 0xff], Vec::<u8>::new()))
                .unwrap(),
            "the binary layout is the types' own"
        );
        assert_eq!(
            SerdeFormat::Bitcode
                .deserialize::<Fields>(&bitcode)
                .unwrap(),
            fields
        );

        let read = |text: &str| SerdeFormat::Ron.deserialize::<Fields>(text.as_bytes());
        assert_eq!(
            read("(digest: \"000FA5FF\", string: \"01\")").unwrap(),
            Fields {
                string: vec![1],
                ..fields
            }
        );
        for bad in ["000fa5f", "000fa5fg", "000fa5", "000fa5ff00"] {
            assert!(
                read(&format!("(digest: {bad:?}, string: \"\")")).is_err(),
                "{bad}"
            );
        }
    }
}
