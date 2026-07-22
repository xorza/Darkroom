use std::io::Cursor;

use crate::TypeId;
use crate::execution::cache::CachedOutputCoverage;
use crate::execution::codec::CodecVersion;
use crate::execution::digest::Digest;
use crate::execution::disk_store::header::{
    FIXED_LEN, FORMAT_VERSION, MAGIC, covers_header, covers_outputs, encode, parse, read_coverage,
};
use crate::library::Library;
use crate::{DynamicValue, StaticValue};

fn raw_header(digest: Digest, coverage: &[u8], codecs: &[CodecVersion]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&digest.0);
    bytes.extend_from_slice(&u32::try_from(coverage.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(codecs.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(coverage);
    for codec in codecs {
        bytes.extend_from_slice(&codec.type_id.as_u128().to_le_bytes());
        bytes.extend_from_slice(&codec.version.to_le_bytes());
    }
    bytes
}

#[test]
fn parse_borrows_exact_header_fields_and_body() {
    let mut type_ids = [TypeId::unique(), TypeId::unique()];
    type_ids.sort_unstable();
    let codecs = [
        CodecVersion {
            type_id: type_ids[0],
            version: 2,
        },
        CodecVersion {
            type_id: type_ids[1],
            version: 5,
        },
    ];
    let digest = Digest([7; 32]);
    let mut bytes = raw_header(digest, &[1, 0, 1], &codecs);
    bytes.extend_from_slice(&[9, 8, 7]);

    let parsed = parse(&bytes).unwrap();
    assert_eq!(parsed.digest, digest);
    assert_eq!(parsed.coverage, [1, 0, 1]);
    assert_eq!(parsed.codecs.entry_count, 2);
    assert_eq!(parsed.codecs.versions().collect::<Vec<_>>(), codecs);
    assert_eq!(parsed.body, [9, 8, 7]);
    assert_eq!(bytes.len(), FIXED_LEN + 3 + 2 * 20 + parsed.body.len());
}

#[test]
fn streaming_validation_compares_coverage_without_owned_header_fields() {
    let digest = Digest([3; 32]);
    let outputs = vec![
        DynamicValue::Static(StaticValue::Int(4)),
        DynamicValue::Unbound,
        DynamicValue::Static(StaticValue::Bool(true)),
    ];
    let library = Library::default();
    let header = encode(digest, &outputs, &library).unwrap();
    let mut blob = header.clone();
    blob.extend_from_slice(&[8, 9]);

    let mut reader = Cursor::new(&blob);
    assert_eq!(
        read_coverage(&mut reader, blob.len() as u64, digest, &library).unwrap(),
        Some(CachedOutputCoverage {
            ports: vec![true, false, true]
        })
    );
    let mut reader = Cursor::new(&blob);
    assert!(covers_outputs(
        &mut reader,
        blob.len() as u64,
        digest,
        &outputs,
        &library
    )
    .unwrap());
    let required_more = vec![
        DynamicValue::Static(StaticValue::Int(4)),
        DynamicValue::Static(StaticValue::Int(5)),
        DynamicValue::Static(StaticValue::Bool(true)),
    ];
    let mut reader = Cursor::new(&blob);
    assert!(!covers_outputs(
        &mut reader,
        blob.len() as u64,
        digest,
        &required_more,
        &library
    )
    .unwrap());
    let mut reader = Cursor::new(&blob);
    assert!(covers_header(
        &mut reader,
        blob.len() as u64,
        &header,
        &library
    )
    .unwrap());
}

#[test]
fn rejects_wrong_identity_version_counts_coverage_manifest_order_and_truncation() {
    let mut type_ids = [TypeId::unique(), TypeId::unique()];
    type_ids.sort_unstable();
    let codecs = [
        CodecVersion {
            type_id: type_ids[0],
            version: 2,
        },
        CodecVersion {
            type_id: type_ids[1],
            version: 5,
        },
    ];
    let original = raw_header(Digest([7; 32]), &[1, 0, 1], &codecs);
    let mut cases = Vec::new();

    let mut wrong_magic = original.clone();
    wrong_magic[0] ^= 0xff;
    cases.push(wrong_magic);

    let mut wrong_version = original.clone();
    wrong_version[MAGIC.len()] = FORMAT_VERSION.wrapping_add(1) as u8;
    cases.push(wrong_version);

    let mut oversized_outputs = original.clone();
    oversized_outputs[44..48].copy_from_slice(&u32::MAX.to_le_bytes());

    let mut invalid_coverage = original.clone();
    invalid_coverage[FIXED_LEN] = 2;
    cases.push(invalid_coverage);

    let codec_start = FIXED_LEN + 3;
    let mut duplicate_codec = original.clone();
    let first_type = duplicate_codec[codec_start..codec_start + 16].to_vec();
    duplicate_codec[codec_start + 20..codec_start + 36].copy_from_slice(&first_type);
    cases.push(duplicate_codec);

    let mut descending_codecs = original.clone();
    let first_codec = descending_codecs[codec_start..codec_start + 20].to_vec();
    let second_codec = descending_codecs[codec_start + 20..codec_start + 40].to_vec();
    descending_codecs[codec_start..codec_start + 20].copy_from_slice(&second_codec);
    descending_codecs[codec_start + 20..codec_start + 40].copy_from_slice(&first_codec);
    cases.push(descending_codecs);

    for bytes in cases {
        assert_eq!(
            parse(&bytes).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }
    assert_eq!(
        parse(&oversized_outputs).unwrap_err().kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    for len in 0..original.len() {
        assert_eq!(
            parse(&original[..len]).unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }
}
