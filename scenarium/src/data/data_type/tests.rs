use super::*;

const IMAGE: FileKindId = FileKindId::from_u128(0x1);
const PNG: FileKindId = FileKindId::from_u128(0x2);

fn file(kind: FileKindId, mode: FileMode) -> DataType {
    DataType::File { kind, mode }
}

/// The wire table with no file kind within another but itself, which is what a library answers
/// for kinds it does not register.
fn wires(input: DataType, source: DataType) -> bool {
    input.compatible_with(source, |source, input| source == input)
}

#[test]
fn compatibility_and_defaults_follow_runtime_coercions() {
    let custom = |id| DataType::Custom(CustomTypeId::from_u128(id));

    assert!(wires(DataType::Float, DataType::Int));
    assert!(wires(DataType::Bool, DataType::Float));
    assert!(wires(DataType::Any, DataType::String));
    assert!(wires(DataType::Any, custom(1)));
    assert!(
        !wires(DataType::String, DataType::Any),
        "an `Any` source is no wildcard: only an `Any` input takes it"
    );
    assert!(!wires(DataType::String, DataType::Bool));
    assert!(wires(custom(1), custom(1)));
    assert!(!wires(custom(1), custom(2)));
    assert!(wires(DataType::Dir, DataType::Dir));
    assert!(!wires(DataType::Dir, file(IMAGE, FileMode::Open)));
    assert!(!wires(file(IMAGE, FileMode::Open), DataType::Dir));
    assert!(!wires(DataType::String, DataType::Dir));

    assert_eq!(
        DataType::Float.default_value(),
        Some(ConstValue::Float(0.0))
    );
    assert_eq!(DataType::Int.default_value(), Some(ConstValue::Int(0)));
    assert_eq!(
        DataType::Bool.default_value(),
        Some(ConstValue::Bool(false))
    );
    for one in [
        DataType::Dir,
        file(IMAGE, FileMode::Open),
        file(IMAGE, FileMode::Save),
    ] {
        assert_eq!(
            one.default_value(),
            Some(ConstValue::FsPath(String::new())),
            "{one:?}"
        );
    }
    assert_eq!(
        file(IMAGE, FileMode::OpenMany).default_value(),
        Some(ConstValue::FsPaths(Vec::new()))
    );
    assert_eq!(custom(1).default_value(), None);
    assert_eq!(DataType::Enum(EnumId::from_u128(2)).default_value(), None);
}

/// A file feeds a file input when the two hold the same kind of value, one path or several, and
/// the source's kind is within the input's, which the caller answers: the answer is asked for
/// the source's kind first, and asked only of two file types of one kind of value.
#[test]
fn a_file_wires_by_its_kind_of_value_and_the_callers_answer() {
    let mut asked = Vec::new();
    let mut within = |input: DataType, source: DataType, answer: bool| {
        input.compatible_with(source, |source, input| {
            asked.push((source, input));
            answer
        })
    };
    assert!(within(
        file(IMAGE, FileMode::Open),
        file(PNG, FileMode::Open),
        true
    ));
    assert!(!within(
        file(IMAGE, FileMode::Open),
        file(PNG, FileMode::Open),
        false
    ));
    assert!(
        within(file(IMAGE, FileMode::Save), file(PNG, FileMode::Open), true),
        "an existing file and a file to write both hold one path"
    );
    assert!(
        !within(
            file(IMAGE, FileMode::OpenMany),
            file(IMAGE, FileMode::Open),
            true
        ),
        "one file does not feed several"
    );
    assert!(
        !within(
            file(IMAGE, FileMode::Open),
            file(IMAGE, FileMode::OpenMany),
            true
        ),
        "nor several one"
    );
    assert_eq!(asked, [(PNG, IMAGE), (PNG, IMAGE), (PNG, IMAGE)]);
}

/// The two strictnesses must actually diverge, and only in the two arms
/// they are documented to: scalars and enums. Every other arm has to agree,
/// which is the property that lets one table serve both gates.
#[test]
fn strictness_changes_the_scalar_and_enum_arms_and_nothing_else() {
    let mode = EnumId::from_u128(0x5ca1_ab1e);
    let known = |_: EnumId, name: &str| name == "fast";
    let declared =
        |ty: DataType, v: &ConstValue| ty.accepts_const(v, Strictness::Declared, |_, _| true);
    let authored = |ty: DataType, v: &ConstValue| ty.accepts_const(v, Strictness::Authored, known);

    // Scalars: a `Bool` literal on an `Int` port is a coercion the runtime
    // performs (`as_i64` reads `true` as 1), so a document may author it —
    // and a *declaration* may not, because nobody chose it there.
    let int = DataType::Int;
    let boolean = ConstValue::Bool(true);
    assert!(
        authored(int, &boolean),
        "the runtime reads this, so a document may write it"
    );
    assert!(
        !declared(int, &boolean),
        "but a declared default is held to its own kind"
    );
    assert_ne!(declared(int, &boolean), authored(int, &boolean));

    // …and the exact kind satisfies both, so `Declared` is narrower rather
    // than merely different.
    for (ty, value) in [
        (DataType::Int, ConstValue::Int(1)),
        (DataType::Float, ConstValue::Float(1.0)),
        (DataType::Bool, ConstValue::Bool(false)),
    ] {
        assert!(
            declared(ty, &value) && authored(ty, &value),
            "{ty:?} takes its own kind either way"
        );
    }

    // Enums: membership is the registry's answer, so only the authored gate
    // asks it. Both still require an *enum* literal.
    let enum_ty = DataType::Enum(mode);
    let unregistered = ConstValue::Enum("slothful".into());
    let registered = ConstValue::Enum("fast".into());
    assert!(
        declared(enum_ty, &unregistered),
        "no registry exists at declaration time"
    );
    assert!(
        !authored(enum_ty, &unregistered),
        "an authored literal must name a variant"
    );
    assert_ne!(
        declared(enum_ty, &unregistered),
        authored(enum_ty, &unregistered)
    );
    assert!(declared(enum_ty, &registered) && authored(enum_ty, &registered));
    assert!(
        !declared(enum_ty, &ConstValue::Int(0)),
        "an enum port takes enum literals only"
    );

    // Every other arm agrees, whichever gate asks. A path is checked for its kind of value
    // alone, not its extension.
    let many = file(IMAGE, FileMode::OpenMany);
    let cases = [
        (DataType::Any, ConstValue::Enum("anything".into()), true),
        (DataType::String, ConstValue::String("s".into()), true),
        (DataType::String, ConstValue::Int(1), false),
        (many, ConstValue::FsPaths(vec!["a.xyz".into()]), true),
        (many, ConstValue::FsPath("a".into()), false),
        (
            file(IMAGE, FileMode::Open),
            ConstValue::FsPath("a.xyz".into()),
            true,
        ),
        (
            file(IMAGE, FileMode::Save),
            ConstValue::FsPaths(vec!["a".into()]),
            false,
        ),
        (DataType::Dir, ConstValue::FsPath("a".into()), true),
        (
            DataType::Custom(CustomTypeId::from_u128(1)),
            ConstValue::String("s".into()),
            false,
        ),
    ];
    for (ty, value, expected) in cases {
        assert_eq!(
            declared(ty, &value),
            expected,
            "declared {ty:?} / {value:?}"
        );
        assert_eq!(
            authored(ty, &value),
            expected,
            "authored {ty:?} / {value:?}"
        );
    }
}

/// An output holds what a read of it as declared takes: the scalar class, a constant of its kind,
/// a custom value of its type, a null only as `Any`, and nothing written as nothing yet.
#[test]
fn an_output_holds_what_a_read_of_it_as_declared_takes() {
    use std::sync::Arc;

    use crate::data::dynamic_value::DynamicValue;
    use crate::internals::blob::{BLOB_TYPE, Blob};

    let blob = DynamicValue::Custom(Arc::new(Blob(Vec::new())));
    let int = DynamicValue::from(1i64);
    let text = DynamicValue::Static(ConstValue::String("t".into()));
    let null = DynamicValue::Static(ConstValue::Null);
    let cases = [
        (DataType::Float, &int, true),
        (DataType::Float, &text, false),
        (DataType::String, &text, true),
        (DataType::Custom(BLOB_TYPE), &blob, true),
        (DataType::Custom(BLOB_TYPE), &int, false),
        (DataType::Custom(CustomTypeId::from_u128(9)), &blob, false),
        (DataType::Int, &blob, false),
        (DataType::Any, &blob, true),
        (DataType::Any, &null, true),
        (DataType::Int, &null, false),
        (DataType::Int, &DynamicValue::Unbound, true),
    ];
    for (data_type, value, held) in cases {
        assert_eq!(value.fits(data_type), held, "{data_type:?} of {value}");
    }
}
