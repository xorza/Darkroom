//! `#[derive(Introspect)]` — generic struct introspection (see
//! `common::introspect`).
//!
//! Generates `impl common::Introspect`: `fields()` (a [`common::FieldDesc`] per
//! struct field — name, label, kind, default, required) and `from_fields()`
//! (rebuild `Self` from neutral [`common::FieldValue`]s, typed, falling back to
//! the field's `Default` on a missing/mismatched value).
//!
//! Field types map to a [`common::FieldKind`]: integers → `Int`, floats →
//! `Float`, `bool` → `Bool`, `String` → `Str`, `Option<T>` → `T` but not
//! required, anything else → an enum (the type must impl
//! `common::IntrospectEnum`). Field attribute `#[config(label = "…")]` overrides
//! the auto label (the field name title-cased).

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Data, DeriveInput, Field, Fields, GenericArgument, Ident, LitStr, PathArguments, Type,
    parse_macro_input,
};

#[proc_macro_derive(Introspect, attributes(config))]
pub fn derive_introspect(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(input).unwrap_or_else(|err| err.to_compile_error().into())
}

fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    let ident = &input.ident;
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
    for (index, field) in named.named.iter().enumerate() {
        let fname = field.ident.as_ref().expect("named field");
        let label = field_label(field)?;
        let kind = classify(&field.ty)?;
        descriptors.push(descriptor(fname, &label, &kind));
        builders.push(builder(index, fname, &kind));
    }

    Ok(quote! {
        impl ::common::Introspect for #ident {
            fn fields() -> ::std::vec::Vec<::common::FieldDesc> {
                let d = <Self as ::core::default::Default>::default();
                ::std::vec![ #(#descriptors),* ]
            }

            fn from_fields(values: &[::common::FieldValue]) -> Self {
                let d = <Self as ::core::default::Default>::default();
                Self { #(#builders),* }
            }
        }
    }
    .into())
}

/// Reflected field kind. `Int`/`Float` carry the concrete numeric type (for the
/// cast), `Enum` the type + its name, `Option` its inner kind + inner type.
enum Kind {
    Int(Type),
    Float(Type),
    Bool,
    Str,
    Enum(Type, String),
    Option(Box<Kind>, Type),
}

fn classify(ty: &Type) -> syn::Result<Kind> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "unsupported introspect field type",
        ));
    };
    let segment = path
        .path
        .segments
        .last()
        .ok_or_else(|| syn::Error::new_spanned(ty, "empty type path"))?;
    match segment.ident.to_string().as_str() {
        "usize" | "u8" | "u16" | "u32" | "u64" | "u128" | "isize" | "i8" | "i16" | "i32"
        | "i64" | "i128" => Ok(Kind::Int(ty.clone())),
        "f32" | "f64" => Ok(Kind::Float(ty.clone())),
        "bool" => Ok(Kind::Bool),
        "String" => Ok(Kind::Str),
        "Option" => {
            let inner = match &segment.arguments {
                PathArguments::AngleBracketed(args) => args.args.iter().find_map(|a| match a {
                    GenericArgument::Type(t) => Some(t),
                    _ => None,
                }),
                _ => None,
            }
            .ok_or_else(|| syn::Error::new_spanned(ty, "Option needs a type argument"))?;
            Ok(Kind::Option(Box::new(classify(inner)?), inner.clone()))
        }
        name => Ok(Kind::Enum(ty.clone(), name.to_string())),
    }
}

/// A `FieldDesc { name, label, kind, default, required }`. `d` (the `Default`
/// instance) is in scope at the call site.
fn descriptor(fname: &Ident, label: &str, kind: &Kind) -> TokenStream2 {
    let name = LitStr::new(&fname.to_string(), Span::call_site());
    let label = LitStr::new(label, Span::call_site());
    let required = !matches!(kind, Kind::Option(..));
    let field_kind = kind_tokens(kind);
    let default = default_tokens(fname, kind);
    quote! {
        ::common::FieldDesc {
            name: #name.to_string(),
            label: #label.to_string(),
            kind: #field_kind,
            default: #default,
            required: #required,
        }
    }
}

fn kind_tokens(kind: &Kind) -> TokenStream2 {
    match kind {
        Kind::Int(_) => quote!(::common::FieldKind::Int),
        Kind::Float(_) => quote!(::common::FieldKind::Float),
        Kind::Bool => quote!(::common::FieldKind::Bool),
        Kind::Str => quote!(::common::FieldKind::Str),
        Kind::Option(inner, _) => {
            let inner = kind_tokens(inner);
            quote!(::common::FieldKind::Option(::std::boxed::Box::new(#inner)))
        }
        Kind::Enum(ty, name) => {
            let name = LitStr::new(name, Span::call_site());
            quote! {
                ::common::FieldKind::Enum {
                    type_name: #name.to_string(),
                    variants: <#ty as ::common::IntrospectEnum>::variants(),
                }
            }
        }
    }
}

/// The default `FieldValue` for `place` (e.g. `d.field`, or `(*v)` for an
/// `Option`'s payload).
fn default_scalar(kind: &Kind, place: TokenStream2) -> TokenStream2 {
    match kind {
        Kind::Int(_) => quote!(::common::FieldValue::Int(#place as i64)),
        Kind::Float(_) => quote!(::common::FieldValue::Float(#place as f64)),
        Kind::Bool => quote!(::common::FieldValue::Bool(#place)),
        Kind::Str => quote!(::common::FieldValue::Str(#place.clone())),
        Kind::Enum(..) => {
            quote!(::common::FieldValue::Enum(::common::IntrospectEnum::to_variant(&#place)))
        }
        Kind::Option(..) => quote!(::common::FieldValue::Null),
    }
}

fn default_tokens(fname: &Ident, kind: &Kind) -> TokenStream2 {
    let Kind::Option(inner, _) = kind else {
        return default_scalar(kind, quote!(d.#fname));
    };
    let some = default_scalar(inner, quote!((*v)));
    quote! {
        match &d.#fname {
            ::core::option::Option::None => ::common::FieldValue::Null,
            ::core::option::Option::Some(v) => #some,
        }
    }
}

/// `#field: <read values[index]>` for the typed rebuild.
fn builder(index: usize, fname: &Ident, kind: &Kind) -> TokenStream2 {
    let read = read_tokens(index, fname, kind);
    quote!(#fname: #read)
}

fn read_tokens(index: usize, fname: &Ident, kind: &Kind) -> TokenStream2 {
    let get = quote!(values.get(#index));
    match kind {
        Kind::Int(ty) => quote! {
            match #get {
                ::core::option::Option::Some(::common::FieldValue::Int(n)) => *n as #ty,
                _ => d.#fname,
            }
        },
        Kind::Float(ty) => quote! {
            match #get {
                ::core::option::Option::Some(::common::FieldValue::Float(f)) => *f as #ty,
                _ => d.#fname,
            }
        },
        Kind::Bool => quote! {
            match #get {
                ::core::option::Option::Some(::common::FieldValue::Bool(b)) => *b,
                _ => d.#fname,
            }
        },
        Kind::Str => quote! {
            match #get {
                ::core::option::Option::Some(::common::FieldValue::Str(s)) => s.clone(),
                _ => d.#fname,
            }
        },
        Kind::Enum(ty, _) => quote! {
            match #get {
                ::core::option::Option::Some(::common::FieldValue::Enum(s)) => {
                    <#ty as ::common::IntrospectEnum>::from_variant(s).unwrap_or(d.#fname)
                }
                _ => d.#fname,
            }
        },
        Kind::Option(inner, inner_ty) => option_read(&get, fname, inner, inner_ty),
    }
}

/// Read a bound optional value as `Some(value)`; anything else keeps the default.
fn option_read(get: &TokenStream2, fname: &Ident, inner: &Kind, inner_ty: &Type) -> TokenStream2 {
    let some = match inner {
        Kind::Int(_) => quote! {
            ::core::option::Option::Some(::common::FieldValue::Int(n)) =>
                ::core::option::Option::Some(*n as #inner_ty),
        },
        Kind::Float(_) => quote! {
            ::core::option::Option::Some(::common::FieldValue::Float(f)) =>
                ::core::option::Option::Some(*f as #inner_ty),
        },
        Kind::Bool => quote! {
            ::core::option::Option::Some(::common::FieldValue::Bool(b)) =>
                ::core::option::Option::Some(*b),
        },
        Kind::Str => quote! {
            ::core::option::Option::Some(::common::FieldValue::Str(s)) =>
                ::core::option::Option::Some(s.clone()),
        },
        Kind::Enum(ty, _) => quote! {
            ::core::option::Option::Some(::common::FieldValue::Enum(s)) =>
                <#ty as ::common::IntrospectEnum>::from_variant(s).map(::core::option::Option::Some).unwrap_or(d.#fname),
        },
        Kind::Option(..) => quote!(),
    };
    quote! {
        match #get {
            #some
            _ => d.#fname,
        }
    }
}

/// `#[config(label = "...")]` on a field, else its name title-cased.
fn field_label(field: &Field) -> syn::Result<String> {
    let mut label = None;
    for attr in &field.attrs {
        if !attr.path().is_ident("config") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("label") {
                label = Some(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else {
                Err(meta.error("expected `label`"))
            }
        })?;
    }
    Ok(label.unwrap_or_else(|| prettify(&field.ident.as_ref().expect("named field").to_string())))
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
