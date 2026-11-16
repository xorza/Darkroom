use std::panic;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::async_lambda;
use crate::data::codec::CustomValueCodec;
use crate::data::codec::error::CodecError;
use crate::data::const_value::ConstValue;
use crate::data::data_type::custom_type::{CustomType, CustomTypeId};
use crate::data::data_type::enum_type::{EnumId, EnumType};
use crate::data::data_type::file_kind::{FileKind, FileKindId};
use crate::data::data_type::{DataType, FileMode};
use crate::data::dynamic_value::{CustomValue, DynamicValue};
use crate::graph::func::Func;
use crate::graph::func::error::InvokeError;
use crate::graph::func::lambda::Invocation;
use crate::graph::identity::FuncId;
use crate::graph::ports::{InputDecl, OutputDecl};
use crate::internals;
use crate::internals::func_invoker::FuncInvoker;
use crate::library::Library;

#[derive(Debug)]
struct StubCodec;

#[async_trait::async_trait]
impl CustomValueCodec for StubCodec {
    fn version(&self) -> u32 {
        0
    }

    async fn encode(
        &self,
        _value: &dyn CustomValue,
        _writer: &mut (dyn AsyncWrite + Unpin + Send),
    ) -> Result<(), CodecError> {
        unreachable!()
    }

    async fn decode(
        &self,
        _reader: &mut (dyn AsyncRead + Unpin + Send),
        _byte_len: u64,
    ) -> Result<Arc<dyn CustomValue>, CodecError> {
        unreachable!()
    }
}

/// Whether `register` panics.
fn refused(register: impl FnOnce()) -> bool {
    panic::catch_unwind(AssertUnwindSafe(register)).is_err()
}

#[test]
fn registration_rejects_duplicate_ids_without_replacing_entries() {
    let func_id = FuncId::unique();
    let mut library = Library::default();
    library.add(internals::stub_func(func_id, "Before").build());
    assert!(refused(|| {
        library.add(internals::stub_func(func_id, "After").build());
    }));
    assert_eq!(library.by_id(func_id).unwrap().name(), "Before");

    let custom = CustomTypeId::unique();
    library.register_custom_type(custom, CustomType::new("Before"));
    assert!(refused(|| {
        library.register_custom_type(custom, CustomType::new("After"));
    }));
    assert_eq!(library.custom_type(custom).unwrap().name(), "Before");

    assert!(refused(|| {
        library.register_enum(EnumId::nil(), EnumType::new("Nil", vec![]));
    }));
    assert_eq!(library.type_count(), 1);
}

/// An identical type registers again as a no-op, which is what lets two libraries sharing a
/// type merge; any difference — name, variants, extensions or codec instance — is a conflict.
/// The codec handle a program keeps is the library's at that moment: a later registration copies
/// the map rather than changing it under the program.
#[test]
fn an_identical_type_registers_again_and_a_conflict_panics() {
    let codec: Arc<dyn CustomValueCodec> = Arc::new(StubCodec);
    let blob = CustomTypeId::unique();
    let mode = EnumId::unique();
    let image = FileKindId::unique();
    let mut library = Library::default();
    let register_all = |library: &mut Library| {
        library.register_custom_type(blob, CustomType::with_codec("Blob", Arc::clone(&codec)));
        library.register_enum(mode, EnumType::new("Mode", vec!["a".into()]));
        library.register_file_kind(image, FileKind::new("Image", ["png"]));
    };
    register_all(&mut library);
    let held = Arc::clone(library.codecs());
    register_all(&mut library);
    assert_eq!(library.type_count(), 3);
    assert!(
        Arc::ptr_eq(&held, library.codecs()),
        "a no-op registration leaves the handle alone"
    );

    assert!(refused(|| library.register_custom_type(
        blob,
        CustomType::with_codec("Other", Arc::clone(&codec))
    )));
    assert!(refused(|| library.register_custom_type(
        blob,
        CustomType::with_codec("Blob", Arc::new(StubCodec))
    )));
    assert!(refused(
        || library.register_custom_type(blob, CustomType::new("Blob"))
    ));
    assert!(refused(
        || library.register_enum(mode, EnumType::new("Mode", vec!["b".into()]))
    ));
    assert!(refused(|| library.register_file_kind(
        image,
        FileKind::new("Image", ["png", "jpg"])
    )));

    let added = CustomTypeId::unique();
    library.register_custom_type(added, CustomType::with_codec("Added", Arc::new(StubCodec)));
    library.register_custom_type(CustomTypeId::unique(), CustomType::new("Plain"));
    assert!(library.codecs().get(added).is_some());
    assert!(library.codecs().get(blob).is_some());
    assert!(
        held.get(added).is_none(),
        "the handle a program kept is unchanged"
    );
}

#[test]
fn a_library_iterates_exactly_its_funcs() {
    let ids = [FuncId::unique(), FuncId::unique()];
    let mut library = Library::default();
    library.add(internals::stub_func(ids[0], "first").build());
    library.add(internals::stub_func(ids[1], "second").build());
    assert_eq!(library.iter().len(), 2);
    assert!((&library).into_iter().all(|func| ids.contains(&func.id())));
    assert!(ids.iter().all(|&id| library.by_id(id).is_some()));
}

/// A func whose `mode` input defaults to the enum variant `default` and whose output is a
/// `kind` file.
fn modal_func(mode: EnumId, default: &str, kind: FileKindId) -> Func {
    internals::stub_func(FuncId::unique(), "modal")
        .input(
            InputDecl::optional("mode", DataType::Enum(mode))
                .default(ConstValue::Enum(default.into())),
        )
        .output(OutputDecl::new(
            "file",
            DataType::File {
                kind,
                mode: FileMode::Save,
            },
        ))
        .build()
}

/// A library is whole when every type its funcs name is registered and every enum default is a
/// variant, in whatever order the funcs and types arrived; `check` names the first gap, each
/// kind of gap on its own.
#[test]
fn check_holds_every_declared_type_registered_and_every_enum_default_a_variant() {
    let (mode, file) = (EnumId::unique(), FileKindId::unique());
    let whole = || {
        let mut library = Library::default();
        library.add(modal_func(mode, "fast", file));
        library.register_enum(
            mode,
            EnumType::new("Mode", vec!["fast".into(), "slow".into()]),
        );
        library.register_file_kind(file, FileKind::new("Text", ["txt"]));
        library
    };
    whole().check();

    let message = |library: Library| {
        *panic::catch_unwind(AssertUnwindSafe(|| library.check()))
            .expect_err("a library with a gap is refused")
            .downcast::<String>()
            .expect("assert! panics carry a String")
    };
    let mut no_kind = Library::default();
    no_kind.add(modal_func(mode, "fast", file));
    no_kind.register_enum(mode, EnumType::new("Mode", vec!["fast".into()]));
    assert!(
        message(no_kind).contains("port \"file\" names File"),
        "the file kind is missing"
    );

    let mut no_enum = Library::default();
    no_enum.add(modal_func(mode, "fast", file));
    no_enum.register_file_kind(file, FileKind::new("Text", ["txt"]));
    assert!(message(no_enum).contains("port \"mode\" names Enum"));

    let mut no_variant = whole();
    no_variant.add(modal_func(mode, "slothful", file));
    assert!(message(no_variant).contains("defaults to \"slothful\", which is not a variant"));
}

/// The wire half of admission: a file feeds a file input of the same kind of value whose kind
/// holds every file the source's does. A kind with no extensions is every file, and a kind the
/// library does not register is within itself alone.
#[test]
fn a_file_feeds_an_input_whose_kind_holds_its_own() {
    let [image, png, any, unknown] = [(); 4].map(|()| FileKindId::unique());
    let mut library = Library::default();
    library.register_file_kind(image, FileKind::new("Image", ["png", "jpg", "tif"]));
    library.register_file_kind(png, FileKind::new("PNG", ["png"]));
    library.register_file_kind(any, FileKind::new("File", Vec::<&str>::new()));
    let open = |kind| DataType::File {
        kind,
        mode: FileMode::Open,
    };
    let many = |kind| DataType::File {
        kind,
        mode: FileMode::OpenMany,
    };
    for (input, source, wires) in [
        (open(image), open(png), true),
        (open(png), open(image), false),
        (open(image), open(image), true),
        (many(image), open(png), false),
        (many(image), many(png), true),
        (open(any), open(image), true),
        (open(image), open(any), false),
        (open(any), open(any), true),
        (DataType::Dir, DataType::Dir, true),
        (DataType::Dir, open(any), false),
        (open(unknown), open(unknown), true),
        (open(any), open(unknown), false),
        (open(unknown), open(png), false),
    ] {
        assert_eq!(
            library.wire_satisfies(input, source),
            wires,
            "{source:?} into {input:?}"
        );
    }
}

/// A constant of a registered enum must name a variant, and one of an enum the library does not
/// register names none; a type the library does not register, of each kind, shows as "unknown
/// type".
#[test]
fn a_type_the_library_lacks_takes_no_constant_and_shows_as_unknown() {
    let (known, unknown) = (EnumId::unique(), EnumId::unique());
    let mut library = Library::default();
    library.register_enum(known, EnumType::new("Mode", vec!["fast".into()]));
    let input = |enum_id| InputDecl::required("mode", DataType::Enum(enum_id));
    let slothful = ConstValue::Enum("slothful".into());
    assert!(library.const_satisfies(&input(known), &ConstValue::Enum("fast".into())));
    assert!(!library.const_satisfies(&input(known), &slothful));
    assert!(!library.const_satisfies(&input(unknown), &slothful));

    assert_eq!(library.type_name(DataType::Enum(known)), "Mode");
    for unknown_type in [
        DataType::Enum(unknown),
        DataType::Custom(CustomTypeId::unique()),
        DataType::File {
            kind: FileKindId::unique(),
            mode: FileMode::Open,
        },
    ] {
        assert_eq!(library.type_name(unknown_type), "unknown type");
        assert!(!library.knows(unknown_type));
    }
    assert_eq!(library.type_name(DataType::Dir), "directory");
    assert!(library.knows(DataType::Dir));
}

/// A kind is within another when the other holds each of its extensions, and no extensions is
/// every file. A registration holds each extension to one spelling.
#[test]
fn a_file_kind_is_within_the_kinds_that_hold_its_extensions() {
    let image = FileKind::new("Image", ["png", "jpg"]);
    let png = FileKind::new("PNG", ["png"]);
    let any = FileKind::new("File", Vec::<&str>::new());
    assert!(png.within(&image));
    assert!(!image.within(&png));
    assert!(image.within(&any));
    assert!(!any.within(&image));
    assert!(any.within(&any));
    for bad in [&["PNG"][..], &[".png"], &[""], &["png", "png"]] {
        assert!(
            refused(|| {
                FileKind::new("Bad", bad.iter().copied());
            }),
            "{bad:?}"
        );
    }
}

/// A func computes through the invoker, and its node state carries between
/// calls the way it does between two runs of one node.
#[tokio::test]
async fn an_invoked_func_keeps_its_node_state_between_calls() -> Result<(), InvokeError> {
    let mut library = Library::default();
    library.add(
        Func::builder(
            FuncId::unique(),
            "sum",
            async_lambda!(|Invocation {
                               state,
                               inputs,
                               outputs,
                               ..
                           }| {
                let total = inputs[0].as_i64().unwrap() + inputs[1].as_i64().unwrap();
                state.set(total);
                outputs[0] = ConstValue::Int(total).into();
                Ok(())
            }),
        )
        .pure()
        .input(InputDecl::required("A", DataType::Int))
        .input(InputDecl::required("B", DataType::Int))
        .output(OutputDecl::new("Sum", DataType::Int))
        .build(),
    );
    let sum = library.by_name("sum").unwrap();
    let int = |value: i64| DynamicValue::Static(ConstValue::Int(value));
    let mut node = FuncInvoker::default();

    let outputs = node.call(sum, [int(2), int(4)]).await?;
    assert_eq!(outputs[0].as_i64().unwrap(), 6);
    assert_eq!(
        node.state::<i64>(),
        Some(&6),
        "the body stashed its result in the node's own state"
    );

    let outputs = node.call(sum, [int(3), int(5)]).await?;
    assert_eq!(outputs[0].as_i64().unwrap(), 8);
    assert_eq!(
        node.state::<i64>(),
        Some(&8),
        "and the second call replaced it"
    );

    Ok(())
}
