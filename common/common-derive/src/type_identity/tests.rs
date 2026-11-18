use syn::{DeriveInput, parse_quote};

use crate::type_identity::{Derive, TypeIdentity, type_label};

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
    assert_eq!(identity.name.value(), "Speed");
    let unnamed: DeriveInput = parse_quote! {
        #[config(type_id = "3effbd19-d4a8-4a9b-a931-78fd0e4f8adb")]
        enum BlendMode { A }
    };
    let identity = TypeIdentity::parse(&unnamed, Derive::IntrospectEnum).unwrap();
    assert_eq!(
        identity.name.value(),
        "Blend Mode",
        "the type's name in words"
    );
}

/// A type name splits into words at each capital that starts one: after a lower-case letter or a
/// digit, and at the last capital of a run before a lower-case letter, so an acronym stays whole.
#[test]
fn type_label_splits_a_type_name_into_words() {
    for (ident, label) in [
        ("Mode", "Mode"),
        ("BlendMode", "Blend Mode"),
        ("CombineMethodChoice", "Combine Method Choice"),
        ("HTTPServer", "HTTP Server"),
        ("ServeHTTP", "Serve HTTP"),
        ("Rgb2Hsv", "Rgb2 Hsv"),
        ("Hdr", "Hdr"),
        ("A", "A"),
    ] {
        assert_eq!(type_label(ident), label, "{ident}");
    }
}
