//! [`TypeIdentity`]: the type-level `#[config]` both derives read.

use std::fmt;

use syn::{DeriveInput, LitStr};

/// Which derive is reading the attribute, for the message that names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Derive {
    Introspect,
    IntrospectEnum,
}

impl fmt::Display for Derive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Introspect => "Introspect",
            Self::IntrospectEnum => "IntrospectEnum",
        })
    }
}

/// A type's `#[config(type_id = "…", name = "…")]`.
#[derive(Debug)]
pub(super) struct TypeIdentity {
    pub(super) type_id: LitStr,
    pub(super) name: Option<LitStr>,
}

impl TypeIdentity {
    /// The type-level `#[config]` of `input`: a required canonical lowercase UUID `type_id`, and
    /// an optional display `name`.
    pub(super) fn parse(input: &DeriveInput, derive: Derive) -> syn::Result<Self> {
        let mut type_id = None;
        let mut name = None;
        for attr in &input.attrs {
            if !attr.path().is_ident("config") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    if name.is_some() {
                        return Err(meta.error("duplicate `name`"));
                    }
                    name = Some(meta.value()?.parse::<LitStr>()?);
                    return Ok(());
                }
                if !meta.path.is_ident("type_id") {
                    return Err(meta.error("expected `type_id` or `name`"));
                }
                if type_id.is_some() {
                    return Err(meta.error("duplicate `type_id`"));
                }
                let value = meta.value()?.parse::<LitStr>()?;
                let raw = value.value();
                let Ok(parsed) = uuid::Uuid::parse_str(&raw) else {
                    return Err(syn::Error::new_spanned(&value, "`type_id` must be a UUID"));
                };
                if parsed.hyphenated().to_string() != raw {
                    return Err(syn::Error::new_spanned(
                        &value,
                        "`type_id` must be a canonical lowercase UUID",
                    ));
                }
                type_id = Some(value);
                Ok(())
            })?;
        }
        let type_id = type_id.ok_or_else(|| {
            syn::Error::new_spanned(
                &input.ident,
                format!("{derive} requires `#[config(type_id = \"…\")]`"),
            )
        })?;
        Ok(Self { type_id, name })
    }
}

#[cfg(test)]
mod tests;
