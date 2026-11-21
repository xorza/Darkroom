//! [`IntSpan`]: a span of integers.

use serde::{Deserialize, Serialize};

use crate::decl::value_domain::span_error::SpanError;

/// A span of integers from a floor, with or without a ceiling: valid by construction, its floor
/// below its ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "IntSpanRepr")]
pub struct IntSpan {
    min: i64,
    max: Option<i64>,
}

impl IntSpan {
    /// At least `min`: a count, an index.
    pub const fn at_least(min: i64) -> Self {
        Self { min, max: None }
    }

    /// From `min` to `max`, both held, `min` below `max`.
    ///
    /// # Errors
    /// [`SpanError::Empty`] unless `min` is below `max`.
    pub const fn between_checked(min: i64, max: i64) -> Result<Self, SpanError> {
        if min < max {
            Ok(Self {
                min,
                max: Some(max),
            })
        } else {
            Err(SpanError::Empty)
        }
    }

    /// [`Self::between_checked`], for ends a declaration writes.
    ///
    /// # Panics
    /// On [`Self::between_checked`]'s error.
    pub const fn between(min: i64, max: i64) -> Self {
        match Self::between_checked(min, max) {
            Ok(span) => span,
            Err(_) => panic!("a span's floor is below its ceiling"),
        }
    }

    pub const fn min(self) -> i64 {
        self.min
    }

    pub const fn max(self) -> Option<i64> {
        self.max
    }

    /// Whether `value` lies in the span, its ends included.
    pub const fn contains(self, value: i64) -> bool {
        value >= self.min
            && match self.max {
                Some(max) => value <= max,
                None => true,
            }
    }

    /// `value` held to the span.
    pub const fn clamp(self, value: i64) -> i64 {
        let floored = if value < self.min { self.min } else { value };
        match self.max {
            Some(max) if floored > max => max,
            _ => floored,
        }
    }
}

/// What a file writes: the ends, which [`IntSpan`]'s decode checks.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntSpanRepr {
    min: i64,
    max: Option<i64>,
}

impl TryFrom<IntSpanRepr> for IntSpan {
    type Error = SpanError;

    fn try_from(IntSpanRepr { min, max }: IntSpanRepr) -> Result<Self, SpanError> {
        match max {
            None => Ok(Self::at_least(min)),
            Some(max) => Self::between_checked(min, max),
        }
    }
}
