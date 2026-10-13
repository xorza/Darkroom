use syn::{DeriveInput, parse_quote};

use crate::type_identity::{Derive, TypeIdentity};

/// Each malformed type-level `#[config]` is refused with the message for
/// its rule, naming the derive that read it; a well-formed one yields its id and name.
#[test]
fn type_identity_refuses_each_malformed_attribute() {
    let refusals: [(DeriveInput, &str); 6] = [
        (
            parse_quote! { enum Mode { A } },
            "IntrospectEnum requires `#[config(type_id = \"…\")]`",
        ),
        (
            parse_quote! { #[config(type_id = "not-a-uuid")] enum Mode { A } },
            "`type_id` must be a UUID",
        ),
        (
            parse_quote! {
                #[config(type_id = "3EFFBD19-D4A8-4A9B-A931-78FD0E4F8ADB")]
                enum Mode { A }
            },
            "`type_id` must be a canonical lowercase UUID",
        ),
        (
            parse_quote! {
                #[config(type_id = "3effbd19-d4a8-4a9b-a931-78fd0e4f8adb")]
                #[config(type_id = "3effbd19-d4a8-4a9b-a931-78fd0e4f8adb")]
                enum Mode { A }
            },
            "duplicate `type_id`",
        ),
        (
            parse_quote! {
                #[config(type_id = "3effbd19-d4a8-4a9b-a931-78fd0e4f8adb", name = "A", name = "B")]
                enum Mode { A }
            },
            "duplicate `name`",
        ),
        (
            parse_quote! { #[config(label = "x")] enum Mode { A } },
            "expected `type_id` or `name`",
        ),
    ];
    for (input, message) in refusals {
        let error = TypeIdentity::parse(&input, Derive::IntrospectEnum)
            .expect_err("the attribute is refused");
        assert_eq!(error.to_string(), message);
    }
    let untagged: DeriveInput = parse_quote! { struct Knobs { a: u8 } };
    assert_eq!(
        TypeIdentity::parse(&untagged, Derive::Introspect)
            .expect_err("the attribute is missing")
            .to_string(),
        "Introspect requires `#[config(type_id = \"…\")]`"
    );

    let input: DeriveInput = parse_quote! {
        #[config(type_id = "3effbd19-d4a8-4a9b-a931-78fd0e4f8adb", name = "Speed")]
        enum Mode { A }
    };
    let identity = TypeIdentity::parse(&input, Derive::IntrospectEnum).unwrap();
    assert_eq!(
        identity.type_id.value(),
        "3effbd19-d4a8-4a9b-a931-78fd0e4f8adb"
    );
    assert_eq!(
        identity.name.map(|name| name.value()).as_deref(),
        Some("Speed")
    );
}
