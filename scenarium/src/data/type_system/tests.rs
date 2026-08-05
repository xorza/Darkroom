use super::*;

#[test]
fn type_id_from_name_is_deterministic_namespaced_and_v5() {
    let namespace_a = TypeId::from_u128(0x11);
    let namespace_b = TypeId::from_u128(0x22);

    assert_eq!(
        TypeId::from_name(namespace_a, "Mode"),
        TypeId::from_name(namespace_a, "Mode")
    );
    assert_ne!(
        TypeId::from_name(namespace_a, "Mode"),
        TypeId::from_name(namespace_a, "Other")
    );
    assert_ne!(
        TypeId::from_name(namespace_a, "Mode"),
        TypeId::from_name(namespace_b, "Mode")
    );

    let id = TypeId::from_name(namespace_a, "Mode");
    assert_eq!((id.as_u128() >> 76) & 0xf, 5);
    assert_ne!(id.as_u128() >> 64, 0);
}

#[test]
fn compatibility_and_defaults_follow_runtime_coercions() {
    let custom = |id| DataType::Custom(TypeId::from_u128(id));

    assert!(DataType::Float.compatible_with(&DataType::Int));
    assert!(DataType::Bool.compatible_with(&DataType::Float));
    assert!(DataType::Any.compatible_with(&DataType::String));
    assert!(!DataType::String.compatible_with(&DataType::Bool));
    assert!(custom(1).compatible_with(&custom(1)));
    assert!(!custom(1).compatible_with(&custom(2)));

    assert_eq!(
        DataType::Float.default_value(),
        Some(ConstValue::Float(0.0))
    );
    assert_eq!(DataType::Int.default_value(), Some(ConstValue::Int(0)));
    assert_eq!(
        DataType::Bool.default_value(),
        Some(ConstValue::Bool(false))
    );
    assert_eq!(
        DataType::FsPath(Arc::new(FsPathConfig::default())).default_value(),
        Some(ConstValue::FsPath(String::new()))
    );
    assert_eq!(
        DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFiles))).default_value(),
        Some(ConstValue::FsPaths(Vec::new()))
    );
    assert_eq!(custom(1).default_value(), None);
    assert_eq!(DataType::Enum(TypeId::from_u128(2)).default_value(), None);
}

/// The two strictnesses must actually diverge, and only in the two arms
/// they are documented to: scalars and enums. Every other arm has to agree,
/// which is the property that lets one table serve both gates.
#[test]
fn strictness_changes_the_scalar_and_enum_arms_and_nothing_else() {
    let mode = TypeId::from_u128(0x5ca1ab1e);
    let known = |_: TypeId, name: &str| name == "fast";
    let declared =
        |ty: &DataType, v: &ConstValue| ty.accepts_const(v, Strictness::Declared, |_, _| true);
    let authored = |ty: &DataType, v: &ConstValue| ty.accepts_const(v, Strictness::Authored, known);

    // Scalars: a `Bool` literal on an `Int` port is a coercion the runtime
    // performs (`as_i64` reads `true` as 1), so a document may author it —
    // and a *declaration* may not, because nobody chose it there.
    let int = DataType::Int;
    let boolean = ConstValue::Bool(true);
    assert!(
        authored(&int, &boolean),
        "the runtime reads this, so a document may write it"
    );
    assert!(
        !declared(&int, &boolean),
        "but a declared default is held to its own kind"
    );
    assert_ne!(declared(&int, &boolean), authored(&int, &boolean));

    // …and the exact kind satisfies both, so `Declared` is narrower rather
    // than merely different.
    for (ty, value) in [
        (DataType::Int, ConstValue::Int(1)),
        (DataType::Float, ConstValue::Float(1.0)),
        (DataType::Bool, ConstValue::Bool(false)),
    ] {
        assert!(
            declared(&ty, &value) && authored(&ty, &value),
            "{ty:?} takes its own kind either way"
        );
    }

    // Enums: membership is the registry's answer, so only the authored gate
    // asks it. Both still require an *enum* literal.
    let enum_ty = DataType::Enum(mode);
    let unregistered = ConstValue::Enum("slothful".into());
    let registered = ConstValue::Enum("fast".into());
    assert!(
        declared(&enum_ty, &unregistered),
        "no registry exists at declaration time"
    );
    assert!(
        !authored(&enum_ty, &unregistered),
        "an authored literal must name a variant"
    );
    assert_ne!(
        declared(&enum_ty, &unregistered),
        authored(&enum_ty, &unregistered)
    );
    assert!(declared(&enum_ty, &registered) && authored(&enum_ty, &registered));
    assert!(
        !declared(&enum_ty, &ConstValue::Int(0)),
        "an enum port takes enum literals only"
    );

    // Every other arm agrees, whichever gate asks.
    let path = DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFiles)));
    let cases = [
        (DataType::Any, ConstValue::Enum("anything".into()), true),
        (DataType::String, ConstValue::String("s".into()), true),
        (DataType::String, ConstValue::Int(1), false),
        (path.clone(), ConstValue::FsPaths(vec!["a".into()]), true),
        (path, ConstValue::FsPath("a".into()), false),
        (
            DataType::FsPath(Arc::new(FsPathConfig::default())),
            ConstValue::FsPath("a".into()),
            true,
        ),
        (
            DataType::Custom(mode),
            ConstValue::String("s".into()),
            false,
        ),
    ];
    for (ty, value, expected) in cases {
        assert_eq!(
            declared(&ty, &value),
            expected,
            "declared {ty:?} / {value:?}"
        );
        assert_eq!(
            authored(&ty, &value),
            expected,
            "authored {ty:?} / {value:?}"
        );
    }
}
