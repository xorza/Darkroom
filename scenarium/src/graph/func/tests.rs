use std::sync::Arc;

use crate::data::const_value::ConstValue;
use crate::data::data_type::{DataType, FsPathConfig, FsPathMode, TypeId};
use crate::graph::func::Func;
use crate::graph::func::error::{FuncValidationError, OverrideRule};
use crate::graph::func::func_input::FuncInput;
use crate::graph::func::func_output::FuncOutput;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::func::value_variant::ValueVariant;
use crate::graph::identity::FuncId;
use crate::graph::node::CacheMode;
use crate::internals;

#[test]
fn validate_rejects_invalid_identities_and_defaults() {
    let nil_input = internals::stub_func(FuncId::unique(), "input").input(FuncInput::required(
        "value",
        DataType::Custom(TypeId::nil()),
    ));
    let nil_output = internals::stub_func(FuncId::unique(), "output")
        .output(FuncOutput::new("value", DataType::Enum(TypeId::nil())));
    let any_output = internals::stub_func(FuncId::unique(), "any")
        .output(FuncOutput::new("value", DataType::Any));
    let invalid = [
        (
            "function id must not be nil".to_owned(),
            internals::stub_func(FuncId::nil(), "nil"),
        ),
        (
            format!(
                "function {:?} input 0 has a nil nominal type id",
                nil_input.id
            ),
            nil_input,
        ),
        (
            format!(
                "function {:?} output 0 has a nil nominal type id",
                nil_output.id
            ),
            nil_output,
        ),
        (
            format!("function {:?} output 0 is declared `Any`", any_output.id),
            any_output,
        ),
    ];

    for (expected, func) in invalid {
        assert_eq!(func.validate().unwrap_err().to_string(), expected);
    }

    // Declared defaults are held to exact kinds (no scalar coercion) —
    // an authoring mismatch fails at registration, not in a document.
    let default_mismatch = |input: FuncInput| {
        let func = internals::stub_func(FuncId::unique(), "default").input(input);
        let expected = format!(
            "function {:?} input 0 declares a default that matches neither its type nor its picker variants",
            func.id
        );
        assert_eq!(func.validate().unwrap_err().to_string(), expected);
    };
    default_mismatch(FuncInput::optional("v", DataType::Int).default(ConstValue::Bool(true)));
    default_mismatch(FuncInput::optional("v", DataType::Bool).default(1.0));
    default_mismatch(
        FuncInput::optional("v", DataType::Custom(TypeId::unique()))
            .default(ConstValue::Enum("preset".into())),
    );
    // Null means "explicitly unset" — only an optional input may declare it.
    default_mismatch(FuncInput::required("v", DataType::Int).default(ConstValue::Null));
    // With picker variants the default must be one of the offered picks.
    default_mismatch(
        FuncInput::optional("v", DataType::Int)
            .variants(vec![ValueVariant::new("one", ConstValue::Int(1))])
            .default(2i64),
    );
    // A single-path literal doesn't satisfy a multi-file picker port.
    default_mismatch(
        FuncInput::optional(
            "v",
            DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFiles))),
        )
        .default(ConstValue::FsPath("a.fits".into())),
    );

    // An override is one hop from an optional input onto a const-only one
    // nothing else overrides.
    let preset = || FuncInput::required("preset", DataType::Int).const_only();
    let config = |target| FuncInput::optional("config", DataType::Int).overrides(target);
    let override_rows = [
        (
            vec![preset(), config(2)],
            1,
            2,
            OverrideRule::TargetOutOfRange,
        ),
        (vec![config(0)], 0, 0, OverrideRule::SelfTarget),
        (
            vec![
                preset(),
                FuncInput::required("config", DataType::Int).overrides(0),
            ],
            1,
            0,
            OverrideRule::RequiredOverride,
        ),
        (
            vec![FuncInput::required("preset", DataType::Int), config(0)],
            1,
            0,
            OverrideRule::WirableTarget,
        ),
        (
            vec![
                preset(),
                FuncInput::optional("middle", DataType::Int)
                    .const_only()
                    .overrides(0),
                config(1),
            ],
            1,
            0,
            OverrideRule::Chain,
        ),
        (
            vec![preset(), config(0), config(0)],
            2,
            0,
            OverrideRule::SharedTarget,
        ),
    ];
    for (inputs, input_idx, target, rule) in override_rows {
        let func = Func::builder(FuncId::unique(), "override", FuncLambda::stub()).inputs(inputs);
        assert_eq!(
            func.validate(),
            Err(FuncValidationError::InvalidOverride {
                func_id: func.id,
                input_idx,
                target,
                rule,
            }),
            "{rule}"
        );
    }

    // Well-formed declarations: exact kinds, a variant member, Null on an
    // optional input, `Any` accepting any literal, and a valid override.
    let ok = Func::builder(FuncId::unique(), "ok", FuncLambda::stub())
        .input(FuncInput::optional("int", DataType::Int).default(2i64))
        .input(FuncInput::optional("any", DataType::Any).default("text"))
        .input(FuncInput::optional("unset", DataType::Int).default(ConstValue::Null))
        .input(
            FuncInput::optional("pick", DataType::Int)
                .variants(vec![ValueVariant::new("one", ConstValue::Int(1))])
                .default(1i64),
        )
        .input(
            FuncInput::optional(
                "paths",
                DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFiles))),
            )
            .default(ConstValue::FsPaths(vec!["a.fits".into()])),
        )
        .input(preset())
        .input(config(5))
        .output(FuncOutput::new("value", DataType::Int))
        .build();
    // `config` (input 6) overrides `preset` (input 5); nothing overrides `config`.
    assert_eq!(ok.overrider_of(5), Some(6));
    assert_eq!(ok.overrider_of(6), None);
}

#[test]
fn a_variant_label_is_its_display_else_its_name() {
    let plain = ValueVariant::new("auto_stf", ConstValue::Int(1));
    assert_eq!(plain.label(), "auto_stf");
    let friendly = plain.display("Auto STF");
    assert_eq!(friendly.label(), "Auto STF");
    assert_eq!(
        friendly.name, "auto_stf",
        "the name is what saved graphs bind"
    );
}

#[test]
fn default_cache_mode_defaults_to_none_and_builder_overrides() {
    // Out of the box a func caches nothing.
    assert_eq!(
        internals::stub_func(FuncId::unique(), "f").default_cache_mode,
        CacheMode::None
    );

    // The builder sets a hotter default; distinct inputs map to distinct
    // stored modes (not a fixed constant).
    for mode in [CacheMode::Ram, CacheMode::Disk, CacheMode::Both] {
        let func = internals::stub_func(FuncId::unique(), "f").default_cache_mode(mode);
        assert_eq!(func.default_cache_mode, mode, "{mode:?} is stored verbatim");
    }
}

#[test]
fn node_events_expose_names_and_arity() {
    let emitter = internals::stub_func(FuncId::unique(), "ticker")
        .event("tick", internals::stub_event())
        .event("tock", internals::stub_event());
    assert_eq!(emitter.events.len(), 2);
    let names: Vec<&str> = emitter.events.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["tick", "tock"]);

    let silent = internals::stub_func(FuncId::unique(), "silent");
    assert!(silent.events.is_empty());
}

/// `build` refuses what `validate` refuses, naming the declaration and the rule;
/// the rules themselves are `validate`'s tests.
#[test]
#[should_panic(expected = "invalid function declaration \"nil\": function id must not be nil")]
fn build_refuses_a_declaration_validate_refuses() {
    internals::stub_func(FuncId::nil(), "nil").build();
}
