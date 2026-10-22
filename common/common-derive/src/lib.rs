//! `#[derive(Introspect)]` — generic struct introspection (see
//! `common::introspect`).
//!
//! Generates `impl common::Introspect`: the `TYPE_ID` and `DISPLAY_NAME` the type-level
//! `#[config(type_id = "…", name = "…")]` gives, `fields()` (a `common::FieldDesc` per
//! struct field — name, label, kind, default, required) and `from_fields()`
//! (rebuild `Self` from neutral `common::FieldValue`s, checking numeric
//! conversions and falling back to the field's `Default` on a
//! missing/mismatched value).
//!
//! Field types map to a `common::FieldKind`: integers → `Int`, floats →
//! `Float`, `bool` → `Bool`, `String` → `Str`, `Option<T>` → `T` but not
//! required, anything else → an enum (the type must impl
//! `common::IntrospectEnum`). Field attribute `#[config(label = "…")]` overrides
//! the auto label (the field name title-cased), and so does the same attribute
//! on an enum variant (the variant's words title-cased). Enum derive requires a stable
//! `#[config(type_id = "…")]` UUID, and owns the variant strings itself — no
//! `Display`/`FromStr` and no derive crate beyond this one, so any crate that
//! already depends on `common` can describe its own config types.

mod kind;
mod type_identity;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Field, Fields, Ident, LitStr, parse_macro_input};

use crate::kind::Kind;
use crate::type_identity::{Derive, TypeIdentity};

#[proc_macro_derive(Introspect, attributes(config))]
pub fn derive_introspect(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input).unwrap_or_else(|err| err.to_compile_error().into())
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let identity = TypeIdentity::parse(input, Derive::Introspect)?;
    let type_id = identity.type_id;
    let display_name = identity
        .name
        .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            ident,
            "Introspect is only for structs",
        ));
    };
    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new_spanned(
            ident,
            "Introspect requires named fields",
        ));
    };

    let mut descriptors = Vec::new();
    let mut builders = Vec::new();
    for (idx, field) in named.named.iter().enumerate() {
        let fname = field.ident.as_ref().expect("named field");
        let label = field_label(field)?;
        let kind = Kind::of(&field.ty)?;
        descriptors.push(descriptor(fname, &label, &kind));
        builders.push(builder(idx, fname, &kind));
    }

    Ok(quote! {
        impl ::common::Introspect for #ident {
            const TYPE_ID: &'static str = #type_id;
            const DISPLAY_NAME: &'static str = #display_name;

            fn fields() -> ::std::vec::Vec<::common::FieldDesc> {
                let d = <Self as ::core::default::Default>::default();
                ::std::vec![ #(#descriptors),* ]
            }

            fn from_fields(
                values: &[::common::FieldValue],
            ) -> ::core::result::Result<Self, ::common::IntrospectError> {
                let d = <Self as ::core::default::Default>::default();
                ::core::result::Result::Ok(Self { #(#builders),* })
            }
        }
    }
    .into())
}

/// `#[derive(IntrospectEnum)]` — the variant-list + string round-trip an enum
/// field needs to be an `Introspect` field. The enum must be fieldless (unit
/// variants only).
///
/// The strings are `snake_case` renderings of the variant names, generated
/// here rather than delegated to `Display`/`FromStr`. They are the enum's
/// **persisted** form — a consumer stores them in saved documents — so owning
/// them keeps a `Display` impl written for humans, or a `serialize_all`
/// attribute on some other derive, from silently changing what is on disk.
/// Renaming a variant still changes its string; that is the one rename this
/// derive cannot absorb.
#[proc_macro_derive(IntrospectEnum, attributes(config))]
pub fn derive_introspect_enum(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_enum(&input).unwrap_or_else(|err| err.to_compile_error().into())
}

fn expand_enum(input: &DeriveInput) -> syn::Result<TokenStream> {
    let ident = &input.ident;
    let identity = TypeIdentity::parse(input, Derive::IntrospectEnum)?;
    if let Some(name) = identity.name {
        return Err(syn::Error::new_spanned(
            name,
            "IntrospectEnum takes its display name from the type",
        ));
    }
    let type_id = identity.type_id;
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            ident,
            "IntrospectEnum is only for enums",
        ));
    };
    let mut variants = Vec::new();
    let mut names = Vec::new();
    let mut labels = Vec::new();
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "IntrospectEnum requires fieldless (unit) variants",
            ));
        }
        let name = snake_case(&variant.ident.to_string());
        let label = config_label(&variant.attrs)?.unwrap_or_else(|| prettify(&name));
        variants.push(&variant.ident);
        names.push(LitStr::new(&name, variant.ident.span()));
        labels.push(LitStr::new(&label, variant.ident.span()));
    }

    Ok(quote! {
        impl ::common::IntrospectEnum for #ident {
            const TYPE_ID: &'static str = #type_id;
            const DISPLAY_NAME: &'static str = ::core::stringify!(#ident);
            const VARIANTS: &'static [&'static str] = &[ #(#names),* ];
            const LABELS: &'static [&'static str] = &[ #(#labels),* ];

            fn to_variant(&self) -> &'static str {
                match self {
                    #( Self::#variants => #names ),*
                }
            }

            fn from_variant(name: &str) -> ::std::option::Option<Self> {
                match name {
                    #( #names => ::core::option::Option::Some(Self::#variants), )*
                    _ => ::core::option::Option::None,
                }
            }
        }
    }
    .into())
}

/// The `#[config(label = "…")]` among a field's or a variant's attributes, if it has one.
fn config_label(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut label = None;
    for attr in attrs {
        if !attr.path().is_ident("config") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("label") {
                return Err(meta.error("expected `label`"));
            }
            if label.is_some() {
                return Err(meta.error("duplicate `label`"));
            }
            label = Some(meta.value()?.parse::<LitStr>()?.value());
            Ok(())
        })?;
    }
    Ok(label)
}

/// A variant name in the `snake_case` the persisted string uses: an underscore
/// before each uppercase letter that starts a new word — one that follows a
/// lowercase letter or digit, or that leads a lowercase run out of an acronym.
fn snake_case(ident: &str) -> String {
    let chars: Vec<char> = ident.chars().collect();
    let mut out = String::with_capacity(chars.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        let starts_word = i > 0
            && c.is_uppercase()
            && (chars[i - 1].is_lowercase()
                || chars[i - 1].is_ascii_digit()
                || chars.get(i + 1).is_some_and(char::is_ascii_lowercase));
        if starts_word {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// A `FieldDesc { name, label, kind, default, required }`. `d` (the `Default`
/// instance) is in scope at the call site.
fn descriptor(fname: &Ident, label: &str, kind: &Kind) -> TokenStream2 {
    let name = LitStr::new(&fname.to_string(), Span::call_site());
    let label = LitStr::new(label, Span::call_site());
    let required = kind.is_required();
    let field_kind = kind.tokens();
    let default = kind.default_value(fname);
    quote! {
        ::common::FieldDesc {
            name: #name,
            label: #label,
            kind: #field_kind,
            default: #default,
            required: #required,
        }
    }
}

/// `#field: <read values[idx]>` for the typed rebuild.
fn builder(idx: usize, fname: &Ident, kind: &Kind) -> TokenStream2 {
    let read = kind.read(idx, fname);
    quote!(#fname: #read)
}

/// `#[config(label = "...")]` on a field, else its name title-cased.
fn field_label(field: &Field) -> syn::Result<String> {
    Ok(config_label(&field.attrs)?
        .unwrap_or_else(|| prettify(&field.ident.as_ref().expect("named field").to_string())))
}

/// `snake_case` → "Title Case".
fn prettify(name: &str) -> String {
    name.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            let first = chars.next().unwrap_or_default().to_uppercase();
            format!("{first}{}", chars.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use syn::{DeriveInput, parse_quote};

    use crate::{config_label, prettify};

    /// A variant names its label once, with `label` and nothing else.
    #[test]
    fn config_label_reads_one_label() {
        let variant = |input: DeriveInput| match input.data {
            syn::Data::Enum(data) => data.variants.into_iter().next().unwrap(),
            _ => unreachable!("the fixtures are enums"),
        };
        let labelled =
            variant(parse_quote! { enum Mode { #[config(label = "Auto STF")] AutoStf } });
        assert_eq!(
            config_label(&labelled.attrs).unwrap().as_deref(),
            Some("Auto STF")
        );
        let plain = variant(parse_quote! { enum Mode { AutoStf } });
        assert_eq!(config_label(&plain.attrs).unwrap(), None);
        for (input, message) in [
            (
                parse_quote! { enum Mode { #[config(name = "x")] A } },
                "expected `label`",
            ),
            (
                parse_quote! { enum Mode { #[config(label = "x", label = "y")] A } },
                "duplicate `label`",
            ),
        ] {
            let error = config_label(&variant(input).attrs).unwrap_err();
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn prettify_title_cases_each_word() {
        assert_eq!(prettify("tile_size"), "Tile Size");
        assert_eq!(prettify("limit"), "Limit");
        assert_eq!(prettify("a__b_"), "A B");
        assert_eq!(prettify(""), "");
    }
}
