use serde::{Deserialize, Serialize};

use super::*;
use crate::serde::serde_format::SerdeFormat;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct TestValue {
    label: String,
    count: u32,
}

#[test]
fn backend_failures_keep_their_typed_variant() {
    assert!(matches!(
        SerdeFormat::Ron.deserialize::<i64>(b"(").unwrap_err(),
        DeserializeError::Ron(_)
    ));
    assert!(matches!(
        SerdeFormat::Bitcode.deserialize::<i64>(&[]).unwrap_err(),
        DeserializeError::Bitcode(_)
    ));
}

/// Each format round-trips, `serialize_into` appends after what the buffer holds the same bytes
/// `serialize` returns, and a byte after the encoding is refused.
#[test]
fn every_format_round_trips_and_appends() {
    let value = TestValue {
        label: "payload".to_string(),
        count: 42,
    };
    for format in SerdeFormat::ALL {
        let bytes = format.serialize(&value).unwrap();
        let back: TestValue = format.deserialize(&bytes).unwrap();
        assert_eq!(back, value, "{format:?}");

        let mut out = b"prefix".to_vec();
        format.serialize_into(&value, &mut out).unwrap();
        assert_eq!(&out[..6], b"prefix", "{format:?}");
        assert_eq!(&out[6..], bytes, "{format:?}");

        let mut trailing = bytes;
        trailing.push(b'x');
        assert!(
            format.deserialize::<TestValue>(&trailing).is_err(),
            "{format:?} accepted a trailing byte"
        );
    }
}

#[test]
fn ron_is_the_pretty_text() {
    let value = TestValue {
        label: "a".to_string(),
        count: 1,
    };
    let bytes = SerdeFormat::Ron.serialize(&value).unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "(\n    label: \"a\",\n    count: 1,\n)"
    );
}
