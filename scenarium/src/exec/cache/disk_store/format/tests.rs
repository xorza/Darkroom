use std::io::Cursor;

use crate::data::const_value::ConstValue;
use crate::data::dynamic_value::DynamicValue;
use crate::exec::cache::digest::Digest;
use crate::exec::cache::disk_store::error::{CodecFormatError, FrameError};
use crate::exec::cache::disk_store::format::{
    BlobReader, BlobWriter, COUNT_OFFSET, DESC_LEN, FIXED_LEN, FORMAT_VERSION, header_len,
};
use crate::graph::func::lambda::OutputDemand;
use crate::graph::identity::NodeId;
use crate::graph::instance_path::InstancePath;
use crate::graph::program_id::ProgramId;
use crate::internals::blob::{Blob, BlobCodec, BlobFault};
use crate::internals::calls::Calls;
use crate::library::Library;

/// The program node every blob here is written for: node 7 of the main graph.
fn node() -> ProgramId {
    ProgramId::from(NodeId::from_u128(7))
}

/// A demand mask over `output_count` outputs with `produced` marked demanded and the rest
/// skipped — the shape the run loop hands the decoder.
fn demand(output_count: usize, produced: &[usize]) -> Vec<OutputDemand> {
    let mut demand = vec![OutputDemand::Skip; output_count];
    for &idx in produced {
        demand[idx] = OutputDemand::Produce;
    }
    demand
}

/// Where a descriptor's payload length sits in it.
const PAYLOAD_LEN_OFFSET: usize = 1 + 3 + 16 + 4;

fn encoded(digest: Digest, outputs: &[DynamicValue], library: &Library) -> Vec<u8> {
    let mut writer = Cursor::new(Vec::new());
    BlobWriter {
        writer: &mut writer,
        program_id: node(),
        digest,
        codecs: library.codecs(),
    }
    .write(outputs)
    .unwrap();
    writer.into_inner()
}

/// One small blob's exact bytes, written out by hand from the layout rather than from the
/// format's own constants. They move only when the layout changes on purpose, which must also
/// bump `FORMAT_VERSION`; any other failure is accidental drift, which would make every blob on
/// disk unreadable or read with the wrong meaning.
#[test]
fn a_small_blob_has_the_pinned_layout() {
    let library = BlobCodec::versioned(7, Calls::default()).library();
    let outputs = [
        DynamicValue::Unbound,
        DynamicValue::Static(ConstValue::Int(-2)),
        DynamicValue::Static(ConstValue::String("ab".into())),
        DynamicValue::from_custom(Blob(vec![10, 11])),
    ];
    let bytes = encoded(Digest([0xab; 32]), &outputs, &library);

    // Int: tag 2, -2 as little-endian two's complement.
    let int: &[u8] = &[2, 0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    // String: tag 4, its byte length, its bytes.
    let string: &[u8] = &[4, 2, 0, 0, 0, 0, 0, 0, 0, b'a', b'b'];
    // The custom payload, as the codec wrote it.
    let custom: &[u8] = &[10, 11];
    let hash = |payload: &[u8]| *blake3::hash(payload).as_bytes();
    #[rustfmt::skip]
    let expected: Vec<u8> = [
        // Magic, format version 12, the digest.
        &b"SCENBLOB"[..],
        &[12, 0, 0, 0],
        &[0xab; 32],
        // The path: one id, node 7 little-endian, then seven nil ids.
        &[1],
        &[7], &[0; 15],
        &[0; 7 * 16],
        // Four outputs.
        &[4, 0, 0, 0],
        // Body length: 9 (Int) + 11 (String) + 2 (Blob).
        &[22, 0, 0, 0, 0, 0, 0, 0],
        // One descriptor per output: tag, three zero bytes, the custom type id (16, little-endian),
        // its codec version (4), the payload length (8), the payload's blake3 hash (32).
        &[0; 32], &hash(&[]),
        &[1, 0, 0, 0], &[0; 16], &[0; 4], &[9, 0, 0, 0, 0, 0, 0, 0], &hash(int),
        &[1, 0, 0, 0], &[0; 16], &[0; 4], &[11, 0, 0, 0, 0, 0, 0, 0], &hash(string),
        &[2, 0, 0, 0],
        // The blob type, 78391861-24da-4368-a3a5-2a6b7a47f112, little-endian.
        &[0x12, 0xf1, 0x47, 0x7a, 0x6b, 0x2a, 0xa5, 0xa3, 0x68, 0x43, 0xda, 0x24, 0x61, 0x18, 0x39, 0x78],
        &[7, 0, 0, 0], &[2, 0, 0, 0, 0, 0, 0, 0], &hash(custom),
        int,
        string,
        custom,
    ]
    .concat();
    assert_eq!(bytes, expected);
}

/// A blob holds the path it was written for, and a reader for another node finds no value in it,
/// as for another digest: a name two paths share serves neither the other's value.
#[test]
fn a_blob_of_another_node_serves_nothing() {
    let library = BlobCodec::versioned(7, Calls::default()).library();
    let digest = Digest([5; 32]);
    let outputs = [DynamicValue::Static(ConstValue::Int(3))];
    let bytes = encoded(digest, &outputs, &library);
    let read = |program_id| {
        let bytes = &bytes;
        let library = &library;
        {
            BlobReader {
                reader: &mut Cursor::new(bytes),
                file_len: bytes.len() as u64,
                program_id,
                digest,
                codecs: library.codecs(),
            }
            .read(&demand(1, &[0]))
            .unwrap()
        }
    };
    assert!(read(node()).is_some(), "its own node reads it");
    let inner = InstancePath::MAIN
        .child(node().node_id())
        .node(NodeId::from_u128(9));
    assert!(read(inner).is_none(), "a node inside it does not");
    assert!(
        read(ProgramId::from(NodeId::from_u128(8))).is_none(),
        "another node does not"
    );
}

#[test]
fn indexed_header_checks_without_body_and_all_values_round_trip() {
    let calls = Calls::default();
    let library = BlobCodec::versioned(7, calls.clone()).library();
    let digest = Digest([3; 32]);
    let first_blob = (0u8..=255)
        .cycle()
        .take(128 * 1024 + 17)
        .collect::<Vec<_>>();
    let second_blob = (0u8..=255)
        .rev()
        .cycle()
        .take(256 * 1024 + 9)
        .collect::<Vec<_>>();
    let outputs = vec![
        DynamicValue::Unbound,
        DynamicValue::Static(ConstValue::Null),
        DynamicValue::Static(ConstValue::Float(f64::from_bits(0x7ff8_0000_0000_0001))),
        DynamicValue::Static(ConstValue::Int(-42)),
        DynamicValue::Static(ConstValue::Bool(true)),
        DynamicValue::Static(ConstValue::String("hello".into())),
        DynamicValue::Static(ConstValue::FsPath("frames/reference.fit".into())),
        DynamicValue::Static(ConstValue::FsPaths(vec![
            "frames/light-1.fit".into(),
            "frames/light-2.fit".into(),
        ])),
        DynamicValue::Static(ConstValue::Enum("Screen".into())),
        DynamicValue::from_custom(Blob(first_blob.clone())),
        DynamicValue::from_custom(Blob(second_blob.clone())),
        DynamicValue::Static(ConstValue::Int(99)),
    ];
    let bytes = encoded(digest, &outputs, &library);
    let header_len = header_len(outputs.len());

    let mut header_only = Cursor::new(&bytes[..header_len]);
    assert!(
        BlobReader {
            reader: &mut header_only,
            file_len: bytes.len() as u64,
            program_id: node(),
            digest,
            codecs: library.codecs()
        }
        .covers_outputs(&outputs)
        .unwrap()
    );
    assert_eq!(header_only.position() as usize, header_len);

    assert!(
        BlobReader {
            reader: &mut Cursor::new(&bytes),
            file_len: bytes.len() as u64,
            program_id: node(),
            digest,
            codecs: library.codecs()
        }
        .read(&demand(outputs.len(), &[0]))
        .unwrap()
        .is_none()
    );
    assert_eq!(calls.count(), 0);

    let mut reader = Cursor::new(&bytes);
    let restored = BlobReader {
        reader: &mut reader,
        file_len: bytes.len() as u64,
        program_id: node(),
        digest,
        codecs: library.codecs(),
    }
    .read(&demand(outputs.len(), &[]))
    .unwrap()
    .unwrap();
    assert_eq!(restored.len(), outputs.len());
    for (actual, expected) in restored.iter().zip(&outputs).take(9) {
        match (actual, expected) {
            (DynamicValue::Unbound, DynamicValue::Unbound) => {}
            (DynamicValue::Static(actual), DynamicValue::Static(expected)) => {
                assert_eq!(actual, expected);
            }
            _ => panic!("restored value kind differs from the encoded value"),
        }
    }
    assert_eq!(
        restored[9].as_custom::<Blob>().unwrap().0.as_slice(),
        first_blob
    );
    assert_eq!(
        restored[10].as_custom::<Blob>().unwrap().0.as_slice(),
        second_blob
    );
    assert_eq!(restored[11].as_i64(), Some(99));
    assert_eq!(calls.count(), 2);
}

#[test]
fn custom_decoder_is_bounded_and_must_consume_its_payload() {
    let digest = Digest([4; 32]);
    let outputs = vec![
        DynamicValue::from_custom(Blob(vec![10, 11, 12])),
        DynamicValue::Static(ConstValue::Int(77)),
    ];
    let calls = Calls::default();
    let complete_library = BlobCodec::versioned(1, calls).library();
    let bytes = encoded(digest, &outputs, &complete_library);
    let restored = BlobReader {
        reader: &mut Cursor::new(&bytes),
        file_len: bytes.len() as u64,
        program_id: node(),
        digest,
        codecs: complete_library.codecs(),
    }
    .read(&demand(outputs.len(), &[]))
    .unwrap()
    .unwrap();
    assert_eq!(
        restored[0].as_custom::<Blob>(),
        Some(&Blob(vec![10, 11, 12]))
    );
    assert_eq!(restored[1].as_i64(), Some(77));

    let underread_calls = Calls::default();
    let underread_library = BlobCodec {
        fault: BlobFault::DecodeUnderReads,
        ..BlobCodec::versioned(1, underread_calls.clone())
    }
    .library();
    let error = BlobReader {
        reader: &mut Cursor::new(&bytes),
        file_len: bytes.len() as u64,
        program_id: node(),
        digest,
        codecs: underread_library.codecs(),
    }
    .read(&demand(outputs.len(), &[]))
    .unwrap_err();
    assert!(matches!(
        error,
        CodecFormatError::Frame(FrameError::PayloadNotConsumed)
    ));
    assert_eq!(underread_calls.count(), 1);

    // The decoder reads none of the custom payload, and its last byte changed: the whole payload
    // is still hashed, so the change is named, not the short read.
    let mut changed_tail = bytes;
    changed_tail[header_len(outputs.len()) + 2] ^= 1;
    let error = BlobReader {
        reader: &mut Cursor::new(&changed_tail),
        file_len: changed_tail.len() as u64,
        program_id: node(),
        digest,
        codecs: underread_library.codecs(),
    }
    .read(&demand(outputs.len(), &[]))
    .unwrap_err();
    assert!(
        matches!(
            error,
            CodecFormatError::Frame(FrameError::PayloadHash { output_idx: 0 })
        ),
        "{error}"
    );
}

/// A payload is held to the hash its descriptor states: one flipped bit, or a tail of zeros as a
/// write cut short by a power loss leaves, is refused, naming the output, though every value
/// still decodes.
#[test]
fn a_payload_that_no_longer_hashes_as_written_is_refused() {
    let digest = Digest([8; 32]);
    let library = BlobCodec::versioned(1, Calls::default()).library();
    let outputs = [
        DynamicValue::Static(ConstValue::Int(5)),
        DynamicValue::from_custom(Blob(vec![1, 2, 3, 4])),
    ];
    let bytes = encoded(digest, &outputs, &library);
    let body = header_len(outputs.len());
    // The Int's payload is 9 bytes, so the blob's 4 follow it.
    let custom = body + 9;

    let mut flipped = bytes.clone();
    flipped[body + 1] ^= 0x10;
    let mut zeroed = bytes;
    zeroed[custom + 2..].fill(0);
    for (bytes, output_idx) in [(&flipped, 0), (&zeroed, 1)] {
        let error = BlobReader {
            reader: &mut Cursor::new(bytes),
            file_len: bytes.len() as u64,
            program_id: node(),
            digest,
            codecs: library.codecs(),
        }
        .read(&demand(outputs.len(), &[]))
        .unwrap_err();
        assert!(
            matches!(&error, CodecFormatError::Frame(FrameError::PayloadHash { output_idx: idx }) if *idx == output_idx),
            "{error}"
        );
    }
}

#[test]
fn descriptors_selectively_validate_codecs_and_coverage() {
    let digest = Digest([5; 32]);
    let calls = Calls::default();
    let registered = BlobCodec::versioned(2, calls).library();
    let outputs = vec![
        DynamicValue::Static(ConstValue::Int(1)),
        DynamicValue::from_custom(Blob(vec![2])),
    ];
    let bytes = encoded(digest, &outputs, &registered);

    assert!(
        BlobReader {
            reader: &mut Cursor::new(&bytes),
            file_len: bytes.len() as u64,
            program_id: node(),
            digest,
            codecs: registered.codecs()
        }
        .covers_outputs(&outputs)
        .unwrap()
    );
    let partial = vec![
        DynamicValue::Static(ConstValue::Int(1)),
        DynamicValue::Unbound,
    ];
    assert!(
        BlobReader {
            reader: &mut Cursor::new(&bytes),
            file_len: bytes.len() as u64,
            program_id: node(),
            digest,
            codecs: registered.codecs()
        }
        .covers_outputs(&partial)
        .unwrap()
    );
    let wrong_kind = vec![
        DynamicValue::from_custom(Blob(vec![])),
        DynamicValue::from_custom(Blob(vec![2])),
    ];
    assert!(
        !BlobReader {
            reader: &mut Cursor::new(&bytes),
            file_len: bytes.len() as u64,
            program_id: node(),
            digest,
            codecs: registered.codecs()
        }
        .covers_outputs(&wrong_kind)
        .unwrap()
    );

    let changed_calls = Calls::default();
    let changed = BlobCodec::versioned(3, changed_calls.clone()).library();
    assert!(
        !BlobReader {
            reader: &mut Cursor::new(&bytes),
            file_len: bytes.len() as u64,
            program_id: node(),
            digest,
            codecs: changed.codecs()
        }
        .covers_outputs(&outputs)
        .unwrap()
    );
    assert_eq!(changed_calls.count(), 0);
}

#[test]
fn malformed_header_lengths_tags_and_const_values_are_rejected() {
    let digest = Digest([6; 32]);
    let outputs = vec![DynamicValue::Static(ConstValue::Bool(true))];
    let library = Library::default();
    let original = encoded(digest, &outputs, &library);

    let corrupt = |edit: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = original.clone();
        edit(&mut bytes);
        bytes
    };
    let length_offset = FIXED_LEN + PAYLOAD_LEN_OFFSET;
    let body_offset = COUNT_OFFSET + 4;
    let cases: [(FrameError, Vec<u8>); 7] = [
        (FrameError::WrongMagic, corrupt(&|b| b[0] ^= 0xff)),
        (
            FrameError::UnsupportedVersion,
            corrupt(&|b| b[8..12].copy_from_slice(&FORMAT_VERSION.wrapping_add(1).to_le_bytes())),
        ),
        (
            FrameError::ReservedBytes,
            corrupt(&|b| b[FIXED_LEN + 1] = 1),
        ),
        (FrameError::UnknownValueTag, corrupt(&|b| b[FIXED_LEN] = 9)),
        (
            FrameError::BodyLenMismatch,
            corrupt(&|b| b[length_offset..length_offset + 8].copy_from_slice(&3u64.to_le_bytes())),
        ),
        (
            FrameError::FileLenMismatch,
            corrupt(&|b| b[body_offset..body_offset + 8].copy_from_slice(&3u64.to_le_bytes())),
        ),
        (FrameError::FileLenMismatch, corrupt(&|b| b.push(0))),
    ];
    for (refusal, bytes) in cases {
        let error = BlobReader {
            reader: &mut Cursor::new(&bytes),
            file_len: bytes.len() as u64,
            program_id: node(),
            digest,
            codecs: library.codecs(),
        }
        .covers_outputs(&outputs)
        .unwrap_err();
        assert!(
            matches!(&error, CodecFormatError::Frame(frame) if *frame == refusal),
            "{refusal}: {error}"
        );
    }

    // A boolean of 2: under the hash it was written with, the change is what is named, though
    // the decode fails on it too; under a hash of the bytes as they stand, as a writer that
    // wrote it would state, the decode's own refusal is.
    let body = header_len(1);
    let mut invalid_bool = original.clone();
    invalid_bool[body + 1] = 2;
    let mut rehashed = invalid_bool.clone();
    let hash = *blake3::hash(&rehashed[body..]).as_bytes();
    rehashed[FIXED_LEN + PAYLOAD_LEN_OFFSET + 8..FIXED_LEN + DESC_LEN].copy_from_slice(&hash);
    for (bytes, refusal) in [
        (&invalid_bool, FrameError::PayloadHash { output_idx: 0 }),
        (&rehashed, FrameError::InvalidBool),
    ] {
        let error = BlobReader {
            reader: &mut Cursor::new(bytes),
            file_len: bytes.len() as u64,
            program_id: node(),
            digest,
            codecs: library.codecs(),
        }
        .read(&demand(outputs.len(), &[]))
        .unwrap_err();
        assert!(
            matches!(&error, CodecFormatError::Frame(frame) if *frame == refusal),
            "{refusal}: {error}"
        );
    }

    assert_eq!(DESC_LEN, 64);
}
