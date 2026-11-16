use crate::data::const_value::ConstValue;
use crate::data::data_type::custom_type::CustomTypeId;
use crate::data::data_type::enum_type::EnumId;
use crate::data::data_type::file_kind::FileKindId;
use crate::data::data_type::{DataType, FileMode};
use crate::graph::func::Func;
use crate::graph::func::error::FuncValidationError;
use crate::graph::func::func_builder::FuncBuilder;
use crate::graph::func::lambda::FuncLambda;
use crate::graph::func::value_variant::ValueVariant;
use crate::graph::identity::{FuncId, PortId};
use crate::graph::node::CacheMode;
use crate::graph::ports::{InputDecl, OutputDecl, PortList, PortsError, numbered_port};
use crate::internals;

const FUNC: FuncId = FuncId::from_u128(7);

fn func() -> FuncBuilder {
    internals::stub_func(FUNC, "func")
}

fn ports(error: PortsError) -> Result<(), FuncValidationError> {
    Err(FuncValidationError::Ports {
        func_id: FUNC,
        error,
    })
}

#[test]
fn validate_rejects_invalid_identities_ports_and_defaults() {
    let int = || InputDecl::required("value", DataType::Int);
    let cases = [
        (
            internals::stub_func(FuncId::nil(), "nil"),
            Err(FuncValidationError::NilId),
        ),
        (
            func().input(
                1,
                InputDecl::required("value", DataType::Custom(CustomTypeId::nil())),
            ),
            ports(PortsError::NilType {
                list: PortList::Inputs,
                idx: 0,
            }),
        ),
        (
            func().output(1, OutputDecl::new("value", DataType::Enum(EnumId::nil()))),
            ports(PortsError::NilType {
                list: PortList::Outputs,
                idx: 0,
            }),
        ),
        (
            func().input(
                1,
                InputDecl::required(
                    "file",
                    DataType::File {
                        kind: FileKindId::nil(),
                        mode: FileMode::Open,
                    },
                ),
            ),
            ports(PortsError::NilType {
                list: PortList::Inputs,
                idx: 0,
            }),
        ),
        (
            func().output(1, OutputDecl::new("value", DataType::Any)),
            Err(FuncValidationError::AnyOutput {
                func_id: FUNC,
                output_idx: 0,
            }),
        ),
        (
            func().pure(),
            Err(FuncValidationError::PureWithoutOutputs { func_id: FUNC }),
        ),
        (
            func().input(0, int()),
            ports(PortsError::NilId {
                list: PortList::Inputs,
                idx: 0,
            }),
        ),
        (
            func().input(1, int()).input(1, int()),
            ports(PortsError::DuplicateId {
                list: PortList::Inputs,
                idx: 1,
                port_id: numbered_port(1),
            }),
        ),
        (
            func().event(2, "tick", internals::stub_event()).event(
                2,
                "tock",
                internals::stub_event(),
            ),
            ports(PortsError::DuplicateId {
                list: PortList::Events,
                idx: 1,
                port_id: numbered_port(2),
            }),
        ),
    ];
    for (func, expected) in cases {
        assert_eq!(func.validate(), expected, "{:?}", func.name);
    }
    // An input and an output may share a number, each unique on its own side.
    assert_eq!(
        func()
            .input(1, int())
            .output(1, OutputDecl::new("value", DataType::Int))
            .validate(),
        Ok(())
    );

    // Declared defaults are held to exact kinds (no scalar coercion) —
    // an authoring mismatch fails at registration, not in a document.
    let default_mismatch = |input: InputDecl| {
        assert_eq!(
            func().input(1, input).validate(),
            ports(PortsError::InvalidDefault { idx: 0 })
        );
    };
    default_mismatch(InputDecl::optional("v", DataType::Int).default(ConstValue::Bool(true)));
    default_mismatch(InputDecl::optional("v", DataType::Bool).default(1.0));
    default_mismatch(
        InputDecl::optional("v", DataType::Custom(CustomTypeId::unique()))
            .default(ConstValue::Enum("preset".into())),
    );
    // Null means "explicitly unset" — only an optional input may declare it.
    default_mismatch(InputDecl::required("v", DataType::Int).default(ConstValue::Null));
    // With picker variants the default must be one of the offered picks.
    default_mismatch(
        InputDecl::optional("v", DataType::Int)
            .variants(vec![ValueVariant::new("one", ConstValue::Int(1))])
            .default(2i64),
    );
    // A single-path literal doesn't satisfy a multi-file picker port.
    let files = DataType::File {
        kind: FileKindId::unique(),
        mode: FileMode::OpenMany,
    };
    default_mismatch(InputDecl::optional("v", files).default(ConstValue::FsPath("a.fits".into())));

    // Well-formed declarations: exact kinds, a variant member, Null on an
    // optional input, `Any` accepting any literal, and a picker whose variants
    // are not of its type, as a preset beside a wired config is.
    let ok = Func::builder(FuncId::unique(), "ok", FuncLambda::stub())
        .input(1, InputDecl::optional("int", DataType::Int).default(2i64))
        .input(2, InputDecl::optional("any", DataType::Any).default("text"))
        .input(
            3,
            InputDecl::optional("unset", DataType::Int).default(ConstValue::Null),
        )
        .input(
            4,
            InputDecl::optional("pick", DataType::Int)
                .variants(vec![ValueVariant::new("one", ConstValue::Int(1))])
                .default(1i64),
        )
        .input(
            5,
            InputDecl::optional("paths", files).default(ConstValue::FsPaths(vec!["a.fits".into()])),
        )
        .input(
            6,
            InputDecl::required("preset", DataType::Bool)
                .variants(vec![ValueVariant::new(
                    "fast",
                    ConstValue::Enum("fast".to_owned()),
                )])
                .default(ConstValue::Enum("fast".to_owned())),
        )
        .output(1, OutputDecl::new("value", DataType::Int));
    assert_eq!(ok.validate(), Ok(()));
}

/// Each port takes the number its builder call gives it, whatever was declared before it.
#[test]
fn a_port_takes_the_number_its_func_gives_it() {
    let func = func()
        .input(3, InputDecl::required("a", DataType::Int))
        .input(1, InputDecl::required("b", DataType::Int))
        .output(7, OutputDecl::new("c", DataType::Int))
        .event(2, "tick", internals::stub_event())
        .build();
    let ids = |ids: &[u128]| {
        ids.iter()
            .copied()
            .map(PortId::from_u128)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        func.inputs()
            .iter()
            .map(|input| input.id)
            .collect::<Vec<_>>(),
        ids(&[3, 1])
    );
    assert_eq!(
        func.outputs()
            .iter()
            .map(|output| output.id)
            .collect::<Vec<_>>(),
        ids(&[7])
    );
    assert_eq!(
        func.events()
            .iter()
            .map(|event| event.id)
            .collect::<Vec<_>>(),
        ids(&[2])
    );
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
        .event(1, "tick", internals::stub_event())
        .event(2, "tock", internals::stub_event());
    assert_eq!(emitter.events.len(), 2);
    let names: Vec<&str> = emitter.events.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["tick", "tock"]);

    let silent = internals::stub_func(FuncId::unique(), "silent");
    assert!(silent.events.is_empty());
}

/// `build` refuses what `validate` refuses, naming the declaration and the rule;
/// the rules themselves are `validate`'s tests.
#[test]
#[should_panic(expected = "invalid function declaration \"func\": function \
                Id(00000000-0000-0000-0000-000000000007) declares a port no declaration may hold: \
                input 0 has the nil id")]
fn build_refuses_a_declaration_validate_refuses() {
    func()
        .input(0, InputDecl::required("value", DataType::Int))
        .build();
}
