use crate::data::const_value::ConstValue;
use crate::data::data_type::DataType;
use crate::data::data_type::Strictness;
use crate::graph::func::value_variant::ValueVariant;
use crate::graph::identity::PortId;
use crate::graph::ports::{InputDecl, Numbered, Ports, PortsError};
use crate::graph::value_domain::ValueDomain;
use crate::graph::value_domain::float_span::FloatSpan;
use crate::graph::value_domain::int_span::IntSpan;
use crate::graph::value_domain::span_error::SpanError;

/// A span's checked constructors refuse an empty span and a non-finite end; the others build one.
#[test]
fn a_span_is_valid_by_construction() {
    assert_eq!(IntSpan::between_checked(1, 1), Err(SpanError::Empty));
    assert_eq!(IntSpan::between_checked(2, 1), Err(SpanError::Empty));
    assert_eq!(
        IntSpan::between_checked(0, 1).map(IntSpan::max),
        Ok(Some(1))
    );
    assert_eq!(
        FloatSpan::between_checked(0.0, f64::NAN),
        Err(SpanError::NotFinite)
    );
    assert_eq!(
        FloatSpan::between_checked(f64::NEG_INFINITY, 0.0),
        Err(SpanError::NotFinite)
    );
    assert_eq!(FloatSpan::between_checked(1.0, 1.0), Err(SpanError::Empty));
    assert_eq!(
        FloatSpan::at_least_checked(f64::NAN),
        Err(SpanError::NotFinite)
    );
    assert_eq!(FloatSpan::at_least(1.0).max(), None);
}

/// Each end is held, past either end is not, a floor alone holds everything above it, and a
/// clamp lands on the nearer end; the fraction of a two-ended span is the value's share of it.
#[test]
fn a_span_holds_its_ends_and_clamps_to_them() {
    let share = FloatSpan::between(0.0, 1.0);
    for (value, held, clamped) in [
        (0.0, true, 0.0),
        (1.0, true, 1.0),
        (0.25, true, 0.25),
        (-0.1, false, 0.0),
        (1.7, false, 1.0),
    ] {
        assert_eq!(share.contains(value), held, "{value}");
        assert_eq!(share.clamp(value), clamped, "{value}");
    }
    assert!(!share.contains(f64::NAN));
    assert_eq!(share.fraction(0.25), Some(0.25));
    assert_eq!(share.fraction(-3.0), Some(0.0));
    assert_eq!(share.fraction(f64::NAN), None);
    // (2.5 - -1) / (4 - -1) = 3.5 / 5 = 0.7.
    assert_eq!(FloatSpan::between(-1.0, 4.0).fraction(2.5), Some(0.7));
    assert_eq!(FloatSpan::at_least(1.0).fraction(3.0), None);

    let count = IntSpan::at_least(0);
    assert!(count.contains(0));
    assert!(count.contains(i64::MAX));
    assert!(!count.contains(-1));
    assert_eq!(count.clamp(-5), 0);
    let dial = IntSpan::between(1, 10);
    assert_eq!((dial.clamp(0), dial.clamp(11), dial.clamp(5)), (1, 10, 5));
}

/// The const gate holds a span: a float input of [0, 1] takes 0 and 1 and refuses -0.1 and 1.1,
/// an authored integer inside it too, as the func reads it as that float; Null still answers by
/// the input's need, and a picker still takes exactly what it offers.
#[test]
fn the_const_gate_holds_the_domain() {
    let alpha =
        InputDecl::optional("Alpha", DataType::Float).float_span(FloatSpan::between(0.0, 1.0));
    let takes = |input: &InputDecl, value: ConstValue, strictness| {
        input.accepts_const(&value, strictness, |_, _| true)
    };
    let declared = Strictness::Declared;
    assert!(takes(&alpha, ConstValue::Float(0.0), declared));
    assert!(takes(&alpha, ConstValue::Float(1.0), declared));
    assert!(!takes(&alpha, ConstValue::Float(-0.1), declared));
    assert!(!takes(&alpha, ConstValue::Float(1.1), declared));
    assert!(takes(&alpha, ConstValue::Int(1), Strictness::Authored));
    assert!(!takes(&alpha, ConstValue::Int(2), Strictness::Authored));
    assert!(
        takes(&alpha, ConstValue::Null, declared),
        "an optional input"
    );

    let count = InputDecl::required("Count", DataType::Int).int_span(IntSpan::at_least(0));
    assert!(takes(&count, ConstValue::Int(0), declared));
    assert!(!takes(&count, ConstValue::Int(-1), declared));
    assert!(
        !takes(&count, ConstValue::Null, declared),
        "a required input"
    );

    let picker = InputDecl::required("Mode", DataType::Int).variants(vec![
        ValueVariant::new("one", ConstValue::Int(1)),
        ValueVariant::new("two", ConstValue::Int(2)),
    ]);
    assert!(takes(&picker, ConstValue::Int(2), declared));
    assert!(!takes(&picker, ConstValue::Int(3), declared));
    assert_eq!(picker.domain.variants().len(), 2);
    assert!(ValueDomain::Any.variants().is_empty());
}

/// A declaration's check refuses a span of the other number type, and a default outside its span
/// as the invalid default it is.
#[test]
fn a_ports_check_holds_the_domain_to_its_input() {
    let check = |input: InputDecl| {
        Ports {
            inputs: vec![Numbered::new(PortId::new(1), input)],
            outputs: Vec::new(),
        }
        .check()
    };
    assert_eq!(
        check(InputDecl::optional("x", DataType::Float).int_span(IntSpan::at_least(0))),
        Err(PortsError::DomainMismatch { idx: 0 })
    );
    assert_eq!(
        check(InputDecl::optional("x", DataType::String).float_span(FloatSpan::at_least(0.0))),
        Err(PortsError::DomainMismatch { idx: 0 })
    );
    assert_eq!(
        check(
            InputDecl::optional("x", DataType::Float)
                .float_span(FloatSpan::between(0.0, 1.0))
                .default(1.5)
        ),
        Err(PortsError::InvalidDefault { idx: 0 })
    );
    assert_eq!(
        check(
            InputDecl::optional("x", DataType::Float)
                .float_span(FloatSpan::between(0.0, 1.0))
                .default(0.5)
        ),
        Ok(())
    );
}

/// A domain round-trips through a file, and a file's empty span or input with no domain stated
/// decode as the checks say: the first refused, the second of no domain.
#[test]
fn a_domain_round_trips_and_a_file_cannot_build_an_empty_span() {
    let input = InputDecl::optional("x", DataType::Float).float_span(FloatSpan::between(0.0, 1.0));
    let text = ron::to_string(&input).unwrap();
    let back: InputDecl = ron::from_str(&text).unwrap();
    assert_eq!(back, input);

    let empty = text.replace("max:Some(1.0)", "max:Some(0.0)");
    assert_ne!(empty, text, "premise: the max is written as {text}");
    assert!(ron::from_str::<InputDecl>(&empty).is_err());
    let int = r#"(name:"n",required:true,data_type:Int,description:None,const_only:false,default_value:None,domain:Int((min:3,max:Some(3))))"#;
    assert!(ron::from_str::<InputDecl>(int).is_err());

    let unstated = r#"(name:"n",required:true,data_type:Int,description:None,const_only:false,default_value:None)"#;
    assert_eq!(
        ron::from_str::<InputDecl>(unstated).unwrap().domain,
        ValueDomain::Any
    );
}
