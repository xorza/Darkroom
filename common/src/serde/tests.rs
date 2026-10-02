use std::io::Cursor;
use std::io::ErrorKind;

use super::*;

#[derive(Debug)]
struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
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

    let err = serialize_into(
        1i64,
        SerdeFormat::Bitcode,
        &mut FailingWriter,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        SerializeError::Write(error) if error.kind() == ErrorKind::BrokenPipe
    ));
}

#[test]
fn lz4_round_trips_ron_payload() {
    let value: Vec<i64> = vec![1, 2, 3, 1000, -42];
    let bytes = serialize(&value, SerdeFormat::Lz4).unwrap();
    let expected_size = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let payload = block::decompress(&bytes[4..], expected_size).unwrap();
    assert_eq!(payload, br"[1,2,3,1000,-42]");
    let back: Vec<i64> = deserialize(&bytes, SerdeFormat::Lz4).unwrap();
    assert_eq!(back, value);
}

#[derive(Debug, PartialEq, Serialize, serde::Deserialize)]
struct TestValue {
    label: String,
    count: u32,
}

#[test]
fn slice_and_reader_dispatch_match_for_every_format() {
    let value = TestValue {
        label: "payload".to_string(),
        count: 42,
    };

    for format in [SerdeFormat::Ron, SerdeFormat::Bitcode, SerdeFormat::Lz4] {
        let bytes = serialize(&value, format).unwrap();
        let direct: TestValue = deserialize(&bytes, format).unwrap();
        let streamed: TestValue =
            deserialize_from(&mut Cursor::new(&bytes), format, &mut Vec::new()).unwrap();
        assert_eq!(direct, value, "direct {format:?}");
        assert_eq!(streamed, value, "reader {format:?}");

        let mut with_trailing_data = bytes;
        with_trailing_data.extend_from_slice(match format {
            SerdeFormat::Ron => b"x",
            SerdeFormat::Bitcode | SerdeFormat::Lz4 => &[0xff],
        });
        let direct: Result<TestValue, _> = deserialize(&with_trailing_data, format);
        let streamed: Result<TestValue, _> = deserialize_from(
            &mut Cursor::new(&with_trailing_data),
            format,
            &mut Vec::new(),
        );
        match (direct, streamed) {
            (Ok(direct), Ok(streamed)) => {
                assert_eq!(direct, streamed, "trailing data for {format:?}");
            }
            (Err(_), Err(_)) => {}
            (direct, streamed) => panic!(
                "slice and reader trailing-data behavior differs for {format:?}: \
                 direct={}, reader={}",
                direct.is_ok(),
                streamed.is_ok()
            ),
        }
    }
}

#[test]
fn lz4_encode_size_boundaries_are_checked() {
    assert_eq!(
        checked_lz4_uncompressed_size(LZ4_MAX_UNCOMPRESSED_SIZE).unwrap(),
        LZ4_MAX_UNCOMPRESSED_SIZE as u32
    );

    let over_limit = LZ4_MAX_UNCOMPRESSED_SIZE + 1;
    let err = checked_lz4_uncompressed_size(over_limit).unwrap_err();
    assert_eq!(
        err,
        Lz4SizeError::Limit {
            size: over_limit,
            limit: LZ4_MAX_UNCOMPRESSED_SIZE,
        }
    );

    if let Ok(over_header) = usize::try_from(u64::from(u32::MAX) + 1) {
        let err = checked_lz4_uncompressed_size(over_header).unwrap_err();
        assert_eq!(err, Lz4SizeError::HeaderCapacity { size: over_header });
    }
}

#[test]
fn lz4_payload_shorter_than_header_errors() {
    for input in [&[][..], &[0][..], &[0, 1][..], &[0, 1, 2][..]] {
        let err = deserialize::<i64>(input, SerdeFormat::Lz4).unwrap_err();
        assert!(
            matches!(
                err,
                DeserializeError::Lz4PayloadTooShort { len } if len == input.len()
            ),
            "unexpected error for {}-byte payload: {err}",
            input.len(),
        );
    }
}

#[test]
fn lz4_oversized_length_prefix_is_rejected() {
    let oversized = 0x7FFF_FFFFu32 as usize;
    let mut input = (oversized as u32).to_le_bytes().to_vec();
    input.extend_from_slice(&[1, 2, 3]);
    let err = deserialize::<i64>(&input, SerdeFormat::Lz4).unwrap_err();
    assert!(matches!(
        err,
        DeserializeError::Lz4Size(Lz4SizeError::Limit {
            size,
            limit: LZ4_MAX_UNCOMPRESSED_SIZE,
        }) if size == oversized
    ));

    let just_over = LZ4_MAX_UNCOMPRESSED_SIZE + 1;
    let input = (just_over as u32).to_le_bytes();
    let err = deserialize::<i64>(&input, SerdeFormat::Lz4).unwrap_err();
    assert!(matches!(
        err,
        DeserializeError::Lz4Size(Lz4SizeError::Limit {
            size,
            limit: LZ4_MAX_UNCOMPRESSED_SIZE,
        }) if size == just_over
    ));
}

#[test]
fn lz4_corrupt_and_mismatched_bodies_return_typed_errors() {
    let mut input = 8u32.to_le_bytes().to_vec();
    input.extend_from_slice(&[0xFF; 16]);
    let err = deserialize::<i64>(&input, SerdeFormat::Lz4).unwrap_err();
    assert!(matches!(err, DeserializeError::Lz4(_)));

    let payload = b"1";
    let expected = payload.len() + 1;
    let mut input = (expected as u32).to_le_bytes().to_vec();
    input.extend_from_slice(&block::compress(payload));
    let err = deserialize::<i64>(&input, SerdeFormat::Lz4).unwrap_err();
    assert!(matches!(
        err,
        DeserializeError::Lz4DecompressedSizeMismatch {
            actual,
            expected: error_expected,
        } if actual == payload.len() && error_expected == expected
    ));
}
