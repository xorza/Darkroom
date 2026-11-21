//! [`ValueDomain`]: the constants an input admits, past what its type holds.
//!
//! - **A picker and a span are cases of one field**, so an input cannot declare both, and the
//!   const gate ([`InputDecl::accepts_const`](crate::InputDecl::accepts_const)) holds each: a
//!   picker admits only the values it offers, a span only the numbers it holds, the number a func
//!   reads off the constant. As Blender's properties carry their limits
//!   ([bpy.props](https://docs.blender.org/api/current/bpy.props.html)). Refused: the range in a
//!   port's description, which no gate read; a span the editor alone knows, which let a file hold
//!   what the field could not show; soft limits beside hard ones, a step and a unit, until a func
//!   needs one.
//! - **A wire delivers any number.** The domain holds a constant; a func copes with a computed
//!   value as it did, as Blender's limits bound the field and not the socket.

pub(crate) mod float_span;
pub(crate) mod int_span;
pub(crate) mod span_error;
pub(crate) mod value_variant;

use serde::{Deserialize, Serialize};

use crate::data::const_value::ConstValue;
use crate::data::data_type::DataType;
use crate::graph::value_domain::float_span::FloatSpan;
use crate::graph::value_domain::int_span::IntSpan;
use crate::graph::value_domain::value_variant::ValueVariant;

/// The constants an input admits, past what its type holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueDomain {
    /// Whatever the type holds.
    #[default]
    Any,
    /// One of a few named values, which the editor offers as a picker.
    Variants(Vec<ValueVariant>),
    /// An integer inside a span.
    Int(IntSpan),
    /// A float inside a span.
    Float(FloatSpan),
}

impl ValueDomain {
    /// The picker's variants, empty for a domain that offers none.
    pub fn variants(&self) -> &[ValueVariant] {
        match self {
            Self::Variants(variants) => variants,
            Self::Any | Self::Int(_) | Self::Float(_) => &[],
        }
    }

    /// Whether the domain admits `value`, as far as it speaks: a picker the values it offers, a
    /// span a number it holds, read as a func reads it; `None` where the domain leaves the answer
    /// to the type, as `Any` does and as a span does for a constant that is no number.
    pub(crate) fn admits(&self, value: &ConstValue) -> Option<bool> {
        match self {
            Self::Any => None,
            Self::Variants(variants) => {
                Some(variants.iter().any(|variant| variant.value == *value))
            }
            Self::Int(span) => value.as_i64().map(|number| span.contains(number)),
            Self::Float(span) => value.as_f64().map(|number| span.contains(number)),
        }
    }

    /// Whether the domain can sit on an input of `data_type`: a span on the number type of its
    /// kind, a picker or `Any` on every type.
    pub(crate) const fn fits(&self, data_type: DataType) -> bool {
        match self {
            Self::Any | Self::Variants(_) => true,
            Self::Int(_) => matches!(data_type, DataType::Int),
            Self::Float(_) => matches!(data_type, DataType::Float),
        }
    }
}

#[cfg(test)]
mod tests;
