use super::*;

#[derive(Debug, PartialEq, Serialize, serde::Deserialize)]
struct TestValue {
    label: String,
    count: u32,
}

#[test]
fn backend_failures_keep_their_typed_variant() {
    assert!(matches!(
        deserialize::<i64>(b"(", SerdeFormat::Ron).unwrap_err(),
        DeserializeError::Ron(_)
    ));
    assert!(matches!(
        deserialize::<i64>(&[], SerdeFormat::Bitcode).unwrap_err(),
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
        let bytes = serialize(&value, format).unwrap();
        let back: TestValue = deserialize(&bytes, format).unwrap();
        assert_eq!(back, value, "{format:?}");

        let mut out = b"prefix".to_vec();
        serialize_into(&value, format, &mut out).unwrap();
        assert_eq!(&out[..6], b"prefix", "{format:?}");
        assert_eq!(&out[6..], bytes, "{format:?}");

        let mut trailing = bytes;
        trailing.push(b'x');
        assert!(
            deserialize::<TestValue>(&trailing, format).is_err(),
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
    let bytes = serialize(&value, SerdeFormat::Ron).unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "(\n    label: \"a\",\n    count: 1,\n)"
    );
}
