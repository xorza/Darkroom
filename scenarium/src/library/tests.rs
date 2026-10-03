use crate::graph::identity::FuncId;
use std::panic;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::FuncOutput;
use crate::graph::func::error::InvokeError;
use crate::graph::func::lambda::Invocation;
use crate::graph::func::{Func, FuncInput};
use crate::library::{Library, TypeEntry};
use crate::testing;
use crate::testing::func_invoker::FuncInvoker;
use crate::{
    CodecError, ConstValue, CustomValue, CustomValueCodec, DataType, DynamicValue, TypeId,
    async_lambda,
};

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

#[test]
fn registration_rejects_duplicate_ids_without_replacing_entries() {
    let func_id = FuncId::unique();
    let mut library = Library::default();
    library.add(testing::stub_func(func_id, "Before"));
    let duplicate_func = panic::catch_unwind(AssertUnwindSafe(|| {
        library.add(testing::stub_func(func_id, "After"));
    }));
    assert!(duplicate_func.is_err());
    assert_eq!(library.by_id(func_id).unwrap().name, "Before");

    let type_id = TypeId::unique();
    library.register_type(type_id, TypeEntry::custom("Before"));
    let duplicate_type = panic::catch_unwind(AssertUnwindSafe(|| {
        library.register_type(type_id, TypeEntry::custom("After"));
    }));
    assert!(duplicate_type.is_err());
    assert_eq!(
        library.type_entry(type_id).unwrap().display_name(),
        "Before"
    );
}

/// An identical type registers again as a no-op, which is what lets two
/// libraries sharing a type merge; any difference — name, variants, codec
/// instance, or kind — is a conflict. The codec handle a program keeps is the
/// library's at that moment: a later registration copies the map rather than
/// changing it under the program.
#[test]
fn an_identical_type_registers_again_and_a_conflict_panics() {
    let codec: Arc<dyn CustomValueCodec> = Arc::new(StubCodec);
    let custom = TypeId::unique();
    let named = TypeId::unique();
    let mut library = Library::default();
    library.register_type(
        custom,
        TypeEntry::custom_with_codec("Blob", Arc::clone(&codec)),
    );
    library.register_type(
        named,
        TypeEntry::enum_with_variants("Mode", vec!["a".into()]),
    );
    let held = Arc::clone(library.codecs());

    library.register_type(
        custom,
        TypeEntry::custom_with_codec("Blob", Arc::clone(&codec)),
    );
    library.register_type(
        named,
        TypeEntry::enum_with_variants("Mode", vec!["a".into()]),
    );
    assert_eq!(library.types().len(), 2);
    assert!(
        Arc::ptr_eq(&held, library.codecs()),
        "a no-op registration leaves the handle alone"
    );

    let conflicts = [
        (
            custom,
            TypeEntry::custom_with_codec("Other", Arc::clone(&codec)),
        ),
        (
            custom,
            TypeEntry::custom_with_codec("Blob", Arc::new(StubCodec)),
        ),
        (custom, TypeEntry::custom("Blob")),
        (
            named,
            TypeEntry::enum_with_variants("Mode", vec!["b".into()]),
        ),
        (named, TypeEntry::custom("Mode")),
    ];
    for (type_id, entry) in conflicts {
        let conflict = panic::catch_unwind(AssertUnwindSafe(|| {
            library.register_type(type_id, entry);
        }));
        assert!(conflict.is_err());
    }

    let added = TypeId::unique();
    library.register_type(
        added,
        TypeEntry::custom_with_codec("Added", Arc::new(StubCodec)),
    );
    assert!(library.codecs().get(added).is_some());
    assert!(library.codecs().get(custom).is_some());
    assert!(
        held.get(added).is_none(),
        "the handle a program kept is unchanged"
    );
    assert!(
        library.codecs().get(named).is_none(),
        "an enum has no codec"
    );
}

/// `add` refuses what `Func::validate` refuses, naming the rule; the rules
/// themselves are `validate`'s tests.
#[test]
#[should_panic(expected = "invalid function declaration: NilId")]
fn add_refuses_a_declaration_validate_refuses() {
    Library::default().add(testing::stub_func(FuncId::nil(), "nil"));
}

/// A func declaring a type as an enum while that type is registered
/// as a *custom* one is a wiring bug, and it is refused whichever of
/// the two arrives first.
///
/// `enum_variants` answers `None` both for "not registered yet" and
/// for "registered as a custom type". Deferred registration makes the
/// first legitimate, so nothing could reject the second: the library
/// accepted the pair, and the contradiction only surfaced later and
/// far quieter, when the enum const failed `const_satisfies` and the
/// input lowered to unbound — indistinguishable from a port the
/// user simply never wired.
#[test]
fn an_enum_declaration_over_a_custom_registration_is_refused_either_order() {
    // Type first, then the func that disagrees with it.
    let type_id = TypeId::unique();
    let mut library = Library::default();
    library.register_type(type_id, TypeEntry::custom("Opaque"));
    let func_after = panic::catch_unwind(AssertUnwindSafe(|| {
        library.add(modal_func(type_id, "fast"));
    }));
    assert!(
        func_after.is_err(),
        "a func declaring a registered custom type as an enum must be refused",
    );
    assert_eq!(
        library.funcs().len(),
        0,
        "the refused func is not installed"
    );

    // …and the reverse order, which is the one deferred registration
    // makes reachable in real hosts.
    let type_id = TypeId::unique();
    let mut library = Library::default();
    library.add(modal_func(type_id, "fast"));
    let type_after = panic::catch_unwind(AssertUnwindSafe(|| {
        library.register_type(type_id, TypeEntry::custom("Opaque"));
    }));
    assert!(
        type_after.is_err(),
        "registering a custom type a func declares as an enum must be refused",
    );
    assert!(
        library.type_entry(type_id).is_none(),
        "the refused type is not installed",
    );
    // The id stays free for the declaration that was actually meant.
    library.register_type(type_id, mode_entry());
    assert!(library.enum_variants(type_id).is_some());
}

/// An *output* declaring the enum counts too — the gate reads both
/// port lists, so a mismatch can't hide on the side with no default.
#[test]
#[should_panic(expected = "declares it as an enum")]
fn an_enum_declared_only_by_an_output_still_blocks_a_custom_registration() {
    let type_id = TypeId::unique();
    let mut library = Library::default();
    library.add(
        testing::stub_func(FuncId::unique(), "emit")
            .output(FuncOutput::new("mode", DataType::Enum(type_id))),
    );
    library.register_type(type_id, TypeEntry::custom("Opaque"));
}

/// A func whose `mode` input defaults to the enum variant `default`.
fn modal_func(type_id: TypeId, default: &str) -> Func {
    testing::stub_func(FuncId::unique(), "modal").input(
        FuncInput::optional("mode", DataType::Enum(type_id))
            .default(ConstValue::Enum(default.into())),
    )
}

fn mode_entry() -> TypeEntry {
    TypeEntry::enum_with_variants("Mode", vec!["fast".into(), "slow".into()])
}

#[test]
fn enum_defaults_must_name_registered_variants_in_either_order() {
    // Type first: `add` gates membership on registration.
    let type_id = TypeId::unique();
    let mut library = Library::default();
    library.register_type(type_id, mode_entry());
    library.add(modal_func(type_id, "fast"));

    // Func first: registering the enum re-checks the funcs already added.
    let mut library = Library::default();
    library.add(modal_func(type_id, "slow"));
    library.register_type(type_id, mode_entry());
}

#[test]
#[should_panic(expected = "not a registered variant")]
fn add_rejects_an_enum_default_naming_no_registered_variant() {
    let type_id = TypeId::unique();
    let mut library = Library::default();
    library.register_type(type_id, mode_entry());
    library.add(modal_func(type_id, "slothful"));
}

/// A refusal has to leave nothing behind.
///
/// The panic alone is not the contract — installing the entry and
/// *then* validating meant the registry kept the very declaration it
/// had just refused, and every later `enum_variants` / `type_name`
/// lookup resolved against it. A caller that catches the panic (a
/// host loading a plugin library, a test) went on with a registry
/// that reported the rejected type as registered, and a second
/// `register_type` for the same id then failed as a *duplicate*.
#[test]
fn register_type_rejecting_an_earlier_enum_default_installs_nothing() {
    let type_id = TypeId::unique();
    let mut library = Library::default();
    library.add(modal_func(type_id, "slothful"));

    let rejected = panic::catch_unwind(AssertUnwindSafe(|| {
        library.register_type(type_id, mode_entry());
    }));
    let message = *rejected
        .expect_err("an unregistered variant must be refused")
        .downcast::<String>()
        .expect("assert! panics carry a String");
    assert!(
        message.contains("not a registered variant"),
        "unexpected panic: {message}",
    );

    assert!(
        library.type_entry(type_id).is_none(),
        "the refused entry must not stay installed",
    );
    assert_eq!(library.enum_variants(type_id), None);
    // The id is still free, so a corrected entry registers normally
    // rather than colliding with the rejected one.
    library.register_type(
        type_id,
        TypeEntry::enum_with_variants("Mode", vec!["slothful".to_string()]),
    );
    assert_eq!(
        library.enum_variants(type_id),
        Some(["slothful".to_string()].as_slice()),
    );
}

#[test]
fn type_entry_kinds_expose_only_valid_codec_attachments() {
    let custom = TypeEntry::custom_with_codec("Custom", Arc::new(StubCodec));
    assert_eq!(custom.display_name(), "Custom");
    assert!(custom.variants().is_none());
    assert!(custom.codec().is_some());

    let variants = vec!["A".to_string()];
    let enum_entry = TypeEntry::enum_with_variants("Enum", variants.clone());
    assert_eq!(enum_entry.display_name(), "Enum");
    assert_eq!(enum_entry.variants(), Some(variants.as_slice()));
    assert!(enum_entry.codec().is_none());

    let mut library = Library::default();
    let custom_id = TypeId::unique();
    library.register_type(custom_id, custom);
    let enum_id = TypeId::unique();
    library.register_type(enum_id, enum_entry);
    assert!(library.codecs().get(custom_id).is_some());
    assert!(library.codecs().get(enum_id).is_none());
}

/// A func reached by id computes, and its node state carries between calls
/// the way it does between two runs of one node.
#[tokio::test]
async fn invoke_by_id_and_index() -> Result<(), InvokeError> {
    // The body stashes what it computed in the node's own state, which is
    // what the second call below reads back.
    let mut library = Library::default();
    library.add(
        Func::new(
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
        .input(FuncInput::required("A", DataType::Int))
        .input(FuncInput::required("B", DataType::Int))
        .output(FuncOutput::new("Sum", DataType::Int)),
    );
    let sum = library.by_name("sum").unwrap().id;
    let sum = library.by_id(sum).unwrap();
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
