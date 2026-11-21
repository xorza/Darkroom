use crate::data::data_type::file_kind::FileKindId;
use crate::data::data_type::{DataType, FileMode};
use crate::decl::func::func_signature::FuncSignature;
use crate::decl::ports::{EventDecl, InputDecl, OutputDecl};
use crate::identity::FuncId;
use crate::internals;

/// One small func's signature, from its bytes written out by hand rather than by the
/// encoder: the domain, then each length-prefixed list — input 3 of kind 2 (Int), output 1 of
/// kind 1 (Float), event 2 — each port id 4 bytes and each type its tag and 16 id bytes.
#[test]
fn a_small_func_has_the_pinned_signature() {
    let func = internals::stub_func(FuncId::from_u128(1), "pinned")
        .input(3, InputDecl::required("value", DataType::Int))
        .output(1, OutputDecl::new("out", DataType::Float))
        .event(2, EventDecl::new("tick"), internals::stub_event())
        .build();
    let count = |n: u64| n.to_le_bytes().to_vec();
    let port = |n: u32| n.to_le_bytes().to_vec();
    let type_id = |n: u128| n.to_le_bytes().to_vec();
    let layout = [
        b"scenarium-func-signature-v4".to_vec(),
        count(1),
        port(3),
        vec![2],
        type_id(0),
        count(1),
        port(1),
        vec![1],
        type_id(0),
        count(1),
        port(2),
    ]
    .concat();
    assert_eq!(
        func.func_ref().signature(),
        FuncSignature(*blake3::hash(&layout).as_bytes())
    );
}

/// The signature changes with what a run reads of a port, its id and its type, a file's
/// kind and mode included, and not with its name or description.
#[test]
fn a_signature_covers_ids_and_types_and_not_names() {
    let file = |kind, mode| DataType::File {
        kind: FileKindId::from_u128(kind),
        mode,
    };
    let signature = |number: u32, input: InputDecl| {
        internals::stub_func(FuncId::from_u128(1), "f")
            .input(number, input)
            .build()
            .func_ref()
            .signature()
    };
    let base = signature(1, InputDecl::required("a", file(1, FileMode::Open)));
    assert_eq!(
        signature(
            1,
            InputDecl::optional("renamed", file(1, FileMode::Open)).description("text")
        ),
        base
    );
    for changed in [
        signature(2, InputDecl::required("a", file(1, FileMode::Open))),
        signature(1, InputDecl::required("a", file(2, FileMode::Open))),
        signature(1, InputDecl::required("a", file(1, FileMode::Save))),
        signature(1, InputDecl::required("a", DataType::Dir)),
    ] {
        assert_ne!(changed, base);
    }
}
