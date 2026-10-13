//! [`Kind`]: what an introspected field is, and the tokens each derive emits for it.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{GenericArgument, Ident, PathArguments, PathSegment, Type};

/// Reflected field kind: a value the field must hold, or an `Option` of one. An `Option` never
/// nests another.
#[derive(Debug)]
pub(super) enum Kind {
    Required(Scalar),
    Optional(Scalar),
}

/// The value a field holds. `Int`/`Float` carry the concrete numeric type, `Enum` its type.
#[derive(Debug)]
pub(super) enum Scalar {
    Int(Type),
    Float(Type),
    Bool,
    Str,
    Enum(Type),
}

impl Kind {
    /// The kind of a field typed `ty`, by the last segment of its path.
    pub(super) fn of(ty: &Type) -> syn::Result<Self> {
        let segment = last_segment(ty)?;
        if segment.ident != "Option" {
            return Ok(Self::Required(Scalar::new(ty, segment)));
        }
        let PathArguments::AngleBracketed(args) = &segment.arguments else {
            return Err(syn::Error::new_spanned(ty, "Option needs a type argument"));
        };
        let inner = args
            .args
            .iter()
            .find_map(|arg| match arg {
                GenericArgument::Type(inner) => Some(inner),
                _ => None,
            })
            .ok_or_else(|| syn::Error::new_spanned(ty, "Option needs a type argument"))?;
        let inner_segment = last_segment(inner)?;
        if inner_segment.ident == "Option" {
            return Err(syn::Error::new_spanned(
                ty,
                "nested Option<Option<_>> is not supported",
            ));
        }
        Ok(Self::Optional(Scalar::new(inner, inner_segment)))
    }

    /// Whether a field of this kind must hold a value: every kind but an `Option`.
    pub(super) const fn is_required(&self) -> bool {
        matches!(self, Self::Required(_))
    }

    /// The `common::FieldKind` this kind describes.
    pub(super) fn tokens(&self) -> TokenStream2 {
        match self {
            Self::Required(scalar) => scalar.tokens(),
            Self::Optional(scalar) => {
                let inner = scalar.tokens();
                quote!(::common::FieldKind::Option({
                    const INNER: ::common::FieldKind = #inner;
                    &INNER
                }))
            }
        }
    }

    /// The default `FieldValue` of field `fname`, read from `d`, the `Default` instance in scope
    /// at the call site.
    pub(super) fn default_value(&self, fname: &Ident) -> TokenStream2 {
        match self {
            Self::Required(scalar) => scalar.default_value(&quote!(d.#fname)),
            Self::Optional(scalar) => {
                let some = scalar.default_value(&quote!((*v)));
                quote! {
                    match &d.#fname {
                        ::core::option::Option::None => ::common::FieldValue::Null,
                        ::core::option::Option::Some(v) => #some,
                    }
                }
            }
        }
    }

    /// Field `fname` read back from `values[index]`, falling back to its default in `d`.
    pub(super) fn read(&self, index: usize, fname: &Ident) -> TokenStream2 {
        let get = quote!(values.get(#index));
        match self {
            Self::Required(scalar) => scalar.read(&get, fname),
            Self::Optional(scalar) => scalar.read_optional(&get, fname),
        }
    }
}

impl Scalar {
    /// The value a type ends in `segment` holds: anything not a number, a `bool` or a `String`
    /// being an enum.
    fn new(ty: &Type, segment: &PathSegment) -> Self {
        match segment.ident.to_string().as_str() {
            "usize" | "u8" | "u16" | "u32" | "u64" | "u128" | "isize" | "i8" | "i16" | "i32"
            | "i64" | "i128" => Self::Int(ty.clone()),
            "f32" | "f64" => Self::Float(ty.clone()),
            "bool" => Self::Bool,
            "String" => Self::Str,
            _ => Self::Enum(ty.clone()),
        }
    }

    fn tokens(&self) -> TokenStream2 {
        match self {
            Self::Int(ty) => {
                quote!(::common::FieldKind::Int(<#ty as ::common::IntrospectInteger>::KIND))
            }
            Self::Float(ty) => {
                quote!(::common::FieldKind::Float(<#ty as ::common::IntrospectFloat>::KIND))
            }
            Self::Bool => quote!(::common::FieldKind::Bool),
            Self::Str => quote!(::common::FieldKind::Str),
            Self::Enum(ty) => {
                quote! {
                    ::common::FieldKind::Enum {
                        type_id: <#ty as ::common::IntrospectEnum>::TYPE_ID,
                        display_name: <#ty as ::common::IntrospectEnum>::DISPLAY_NAME,
                        variants: <#ty as ::common::IntrospectEnum>::VARIANTS,
                    }
                }
            }
        }
    }

    /// The `FieldValue` of `place` (e.g. `d.field`, or `(*v)` for an `Option`'s payload).
    fn default_value(&self, place: &TokenStream2) -> TokenStream2 {
        match self {
            Self::Int(ty) => quote! {
                ::common::FieldValue::Int(
                    <#ty as ::core::convert::Into<::common::IntegerValue>>::into(#place)
                )
            },
            Self::Float(ty) => quote! {
                ::common::FieldValue::Float(
                    <#ty as ::common::IntrospectFloat>::into_field_value(#place)
                )
            },
            Self::Bool => quote!(::common::FieldValue::Bool(#place)),
            Self::Str => quote!(::common::FieldValue::Str(#place.clone())),
            Self::Enum(_) => {
                quote!(::common::FieldValue::Enum(::std::string::ToString::to_string(
                    ::common::IntrospectEnum::to_variant(&#place)
                )))
            }
        }
    }

    /// A required field `fname` read from `get`: a bound value of this kind, else the default.
    fn read(&self, get: &TokenStream2, fname: &Ident) -> TokenStream2 {
        match self {
            Self::Int(ty) => quote! {
                match #get {
                    ::core::option::Option::Some(::common::FieldValue::Int(n)) => {
                        <#ty as ::common::IntrospectInteger>::from_field_value(
                            ::core::stringify!(#fname),
                            *n,
                        )?
                    }
                    _ => d.#fname,
                }
            },
            Self::Float(ty) => quote! {
                match #get {
                    ::core::option::Option::Some(::common::FieldValue::Float(f)) => {
                        <#ty as ::common::IntrospectFloat>::from_field_value(
                            ::core::stringify!(#fname),
                            *f,
                        )?
                    }
                    _ => d.#fname,
                }
            },
            Self::Bool => quote! {
                match #get {
                    ::core::option::Option::Some(::common::FieldValue::Bool(b)) => *b,
                    _ => d.#fname,
                }
            },
            Self::Str => quote! {
                match #get {
                    ::core::option::Option::Some(::common::FieldValue::Str(s)) => s.clone(),
                    _ => d.#fname,
                }
            },
            Self::Enum(ty) => quote! {
                match #get {
                    ::core::option::Option::Some(::common::FieldValue::Enum(s)) => {
                        <#ty as ::common::IntrospectEnum>::from_variant(s).unwrap_or(d.#fname)
                    }
                    _ => d.#fname,
                }
            },
        }
    }

    /// An `Option` field `fname` read from `get`: null as `None`, a bound value of this kind as
    /// `Some(value)`, and anything else keeps the default.
    fn read_optional(&self, get: &TokenStream2, fname: &Ident) -> TokenStream2 {
        let some = match self {
            Self::Int(ty) => quote! {
                ::core::option::Option::Some(::common::FieldValue::Int(n)) =>
                    ::core::option::Option::Some(
                        <#ty as ::common::IntrospectInteger>::from_field_value(
                            ::core::stringify!(#fname),
                            *n,
                        )?
                    ),
            },
            Self::Float(ty) => quote! {
                ::core::option::Option::Some(::common::FieldValue::Float(f)) =>
                    ::core::option::Option::Some(
                        <#ty as ::common::IntrospectFloat>::from_field_value(
                            ::core::stringify!(#fname),
                            *f,
                        )?
                    ),
            },
            Self::Bool => quote! {
                ::core::option::Option::Some(::common::FieldValue::Bool(b)) =>
                    ::core::option::Option::Some(*b),
            },
            Self::Str => quote! {
                ::core::option::Option::Some(::common::FieldValue::Str(s)) =>
                    ::core::option::Option::Some(s.clone()),
            },
            Self::Enum(ty) => quote! {
                ::core::option::Option::Some(::common::FieldValue::Enum(s)) =>
                    <#ty as ::common::IntrospectEnum>::from_variant(s).map(::core::option::Option::Some).unwrap_or(d.#fname),
            },
        };
        quote! {
            match #get {
                ::core::option::Option::Some(::common::FieldValue::Null) =>
                    ::core::option::Option::None,
                #some
                _ => d.#fname,
            }
        }
    }
}

/// The last segment of `ty`'s path, the one a field's kind is read from.
fn last_segment(ty: &Type) -> syn::Result<&PathSegment> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "unsupported introspect field type",
        ));
    };
    path.path
        .segments
        .last()
        .ok_or_else(|| syn::Error::new_spanned(ty, "empty type path"))
}

#[cfg(test)]
mod tests {
    use syn::{Type, parse_quote};

    use crate::kind::{Kind, Scalar};

    /// Each field type classifies by its last path segment, an `Option` wrapping the kind of its
    /// argument; a type the derive cannot reflect is refused with the message for its rule.
    #[test]
    fn of_classifies_each_field_type() {
        let kind = |ty: Type| Kind::of(&ty).expect("the type is reflected");
        assert!(matches!(
            kind(parse_quote!(u32)),
            Kind::Required(Scalar::Int(ty)) if ty == parse_quote!(u32)
        ));
        assert!(matches!(
            kind(parse_quote!(::core::primitive::f64)),
            Kind::Required(Scalar::Float(ty)) if ty == parse_quote!(::core::primitive::f64)
        ));
        assert!(matches!(
            kind(parse_quote!(bool)),
            Kind::Required(Scalar::Bool)
        ));
        assert!(matches!(
            kind(parse_quote!(String)),
            Kind::Required(Scalar::Str)
        ));
        assert!(matches!(
            kind(parse_quote!(crate::Mode)),
            Kind::Required(Scalar::Enum(ty)) if ty == parse_quote!(crate::Mode)
        ));
        assert!(matches!(
            kind(parse_quote!(Option<i8>)),
            Kind::Optional(Scalar::Int(ty)) if ty == parse_quote!(i8)
        ));
        assert!(matches!(
            kind(parse_quote!(std::option::Option<Mode>)),
            Kind::Optional(Scalar::Enum(ty)) if ty == parse_quote!(Mode)
        ));
        assert!(kind(parse_quote!(u8)).is_required());
        assert!(!kind(parse_quote!(Option<u8>)).is_required());

        let refusals: [(Type, &str); 5] = [
            (parse_quote!(&str), "unsupported introspect field type"),
            (parse_quote!(Option), "Option needs a type argument"),
            (parse_quote!(Option<'a>), "Option needs a type argument"),
            (
                parse_quote!(Option<[u8; 4]>),
                "unsupported introspect field type",
            ),
            (
                parse_quote!(Option<Option<u8>>),
                "nested Option<Option<_>> is not supported",
            ),
        ];
        for (ty, message) in refusals {
            let error = Kind::of(&ty).expect_err("the type is refused");
            assert_eq!(error.to_string(), message);
        }
    }
}
