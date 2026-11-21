use std::io::{Read, Write};
use std::panic;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use crate::data::codec::CustomValueCodec;
use crate::data::codec::error::CodecError;
use crate::data::const_value::ConstValue;
use crate::data::data_type::custom_type::{CustomType, CustomTypeId};
use crate::data::data_type::enum_type::{EnumId, EnumType};
use crate::data::data_type::file_kind::{FileKind, FileKindId};
use crate::data::data_type::{DataType, FileMode};
use crate::data::dynamic_value::CustomValue;
use crate::graph::func::Func;
use crate::graph::identity::FuncId;
use crate::graph::ports::{InputDecl, OutputDecl};
use crate::internals;
use crate::library::Library;

#[derive(Debug)]
struct StubCodec;

impl CustomValueCodec for StubCodec {
    fn version(&self) -> u32 {
        0
    }

    fn encode(&self, _value: &dyn CustomValue, _writer: &mut dyn Write) -> Result<(), CodecError> {
        unreachable!()
    }

    fn decode(
        &self,
        _reader: &mut dyn Read,
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
        library.register_enum(EnumId::nil(), EnumType::new("Nil", Vec::<String>::new()));
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
        library.register_enum(mode, EnumType::new("Mode", ["a"]));
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
        || library.register_enum(mode, EnumType::new("Mode", ["b"]))
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
            1,
            InputDecl::optional("mode", DataType::Enum(mode))
                .default(ConstValue::Enum(default.into())),
        )
        .output(
            1,
            OutputDecl::new(
                "file",
                DataType::File {
                    kind,
                    mode: FileMode::Save,
                },
            ),
        )
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
        library.register_enum(mode, EnumType::new("Mode", ["fast", "slow"]));
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
    no_kind.register_enum(mode, EnumType::new("Mode", ["fast"]));
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

/// What the library cannot tell about a type it does not register: a constant of an unknown
/// enum is kept unchecked, while a registered enum's constant must name a variant; each kind
/// shows as "Unknown Type".
#[test]
fn a_type_the_library_lacks_is_kept_and_shown_as_unknown() {
    let (known, unknown) = (EnumId::unique(), EnumId::unique());
    let mut library = Library::default();
    library.register_enum(known, EnumType::new("Mode", ["fast"]));
    let input = |enum_id| InputDecl::required("mode", DataType::Enum(enum_id));
    let slothful = ConstValue::Enum("slothful".into());
    assert!(library.const_satisfies(&input(known), &ConstValue::Enum("fast".into())));
    assert!(!library.const_satisfies(&input(known), &slothful));
    assert!(library.const_satisfies(&input(unknown), &slothful));
    assert!(
        !library.const_satisfies(&input(unknown), &ConstValue::Int(1)),
        "an unknown enum still takes an enum literal alone"
    );

    assert_eq!(library.type_name(DataType::Enum(known)), "Mode");
    for unknown_type in [
        DataType::Enum(unknown),
        DataType::Custom(CustomTypeId::unique()),
        DataType::File {
            kind: FileKindId::unique(),
            mode: FileMode::Open,
        },
    ] {
        assert_eq!(library.type_name(unknown_type), "Unknown Type");
        assert!(!library.knows(unknown_type));
    }
    for (data_type, name) in [
        (DataType::Any, "Any"),
        (DataType::Float, "Float"),
        (DataType::Int, "Integer"),
        (DataType::Bool, "Boolean"),
        (DataType::String, "String"),
        (DataType::Dir, "Directory"),
    ] {
        assert_eq!(library.type_name(data_type), name);
    }
    assert!(library.knows(DataType::Dir));
}

/// An input starts with its declared default, else its type's: an enum's first variant, an empty
/// text for `Any`, a scalar's zero; none for a custom type or an enum the library lacks. Whether
/// it takes a constant at all answers the same, without building one.
#[test]
fn an_input_defaults_to_its_declared_value_else_its_types() {
    use crate::data::data_type::custom_type::CustomTypeId;
    use crate::data::data_type::enum_type::{EnumId, EnumType};

    let mut library = Library::default();
    let mode = EnumId::from_u128(1);
    library.register_enum(mode, EnumType::new("Mode", ["Fast", "Fine"]));
    let input = |data_type: DataType| InputDecl::optional("x", data_type);
    let declared = InputDecl {
        default_value: Some(ConstValue::Int(5)),
        ..input(DataType::Int)
    };
    let cases = [
        (declared, Some(ConstValue::Int(5))),
        (
            input(DataType::Enum(mode)),
            Some(ConstValue::Enum("Fast".into())),
        ),
        (input(DataType::Enum(EnumId::from_u128(2))), None),
        (
            input(DataType::Any),
            Some(ConstValue::String(String::new())),
        ),
        (input(DataType::Int), Some(ConstValue::Int(0))),
        (input(DataType::Float), Some(ConstValue::Float(0.0))),
        (input(DataType::Custom(CustomTypeId::from_u128(3))), None),
    ];
    for (port, expected) in cases {
        assert_eq!(
            library.default_const(&port),
            expected,
            "{:?}",
            port.data_type
        );
        assert_eq!(
            library.takes_const(&port),
            expected.is_some(),
            "{:?} takes a constant exactly when it has a default",
            port.data_type
        );
    }
}
