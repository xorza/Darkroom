//! [`FloatSpan`]: a span of finite floats.

use serde::{Deserialize, Serialize};

use crate::decl::value_domain::span_error::SpanError;

/// A span of finite floats from a floor, with or without a ceiling: valid by construction, its
/// ends finite and its floor below its ceiling.
///
/// Its ends compare by their bits, as [`ConstValue`](crate::ConstValue) compares a float, so a
/// declaration that holds one keeps its `Eq`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(try_from = "FloatSpanRepr")]
pub struct FloatSpan {
    min: f64,
    max: Option<f64>,
}

impl PartialEq for FloatSpan {
    fn eq(&self, other: &Self) -> bool {
        self.min.to_bits() == other.min.to_bits()
            && self.max.map(f64::to_bits) == other.max.map(f64::to_bits)
    }
}

impl Eq for FloatSpan {}

impl FloatSpan {
    /// At least `min`.
    ///
    /// # Errors
    /// [`SpanError::NotFinite`] for a NaN or infinite `min`.
    pub const fn at_least_checked(min: f64) -> Result<Self, SpanError> {
        if min.is_finite() {
            Ok(Self { min, max: None })
        } else {
            Err(SpanError::NotFinite)
        }
    }

    /// [`Self::at_least_checked`], for an end a declaration writes.
    ///
    /// # Panics
    /// On [`Self::at_least_checked`]'s error.
    pub const fn at_least(min: f64) -> Self {
        match Self::at_least_checked(min) {
            Ok(span) => span,
            Err(_) => panic!("a span's floor is finite"),
        }
    }

    /// From `min` to `max`, both held: a share, a strength.
    ///
    /// # Errors
    /// [`SpanError::NotFinite`] for a NaN or infinite end, else [`SpanError::Empty`] unless
    /// `min` is below `max`.
    pub const fn between_checked(min: f64, max: f64) -> Result<Self, SpanError> {
        if !min.is_finite() || !max.is_finite() {
            return Err(SpanError::NotFinite);
        }
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
    pub const fn between(min: f64, max: f64) -> Self {
        match Self::between_checked(min, max) {
            Ok(span) => span,
            Err(_) => panic!("a span's ends are finite, its floor below its ceiling"),
        }
    }

    pub const fn min(self) -> f64 {
        self.min
    }

    pub const fn max(self) -> Option<f64> {
        self.max
    }

    /// Whether `value` lies in the span, its ends included; NaN and an infinity lie in none, as
    /// the span holds finite floats alone.
    pub const fn contains(self, value: f64) -> bool {
        value.is_finite()
            && value >= self.min
            && match self.max {
                Some(max) => value <= max,
                None => true,
            }
    }

    /// `value` held to the span; NaN stays NaN, which no span holds.
    pub const fn clamp(self, value: f64) -> f64 {
        let floored = if value < self.min { self.min } else { value };
        match self.max {
            Some(max) if floored > max => max,
            _ => floored,
        }
    }

    /// How far along a span with both ends `value` sits, 0 at the floor and 1 at the ceiling,
    /// held to that; `None` for a span with no ceiling, which has no far end to measure to, and
    /// for NaN, which sits nowhere.
    pub fn fraction(self, value: f64) -> Option<f64> {
        let max = self.max?;
        if value.is_nan() {
            return None;
        }
        // In halves, so a span past half the range of `f64` measures no infinite length.
        let along = self.clamp(value) / 2.0 - self.min / 2.0;
        Some((along / (max / 2.0 - self.min / 2.0)).clamp(0.0, 1.0))
    }
}

/// What a file writes: the ends, which [`FloatSpan`]'s decode checks.
#[derive(Deserialize)]
struct FloatSpanRepr {
    min: f64,
    max: Option<f64>,
}

impl TryFrom<FloatSpanRepr> for FloatSpan {
    type Error = SpanError;

    fn try_from(FloatSpanRepr { min, max }: FloatSpanRepr) -> Result<Self, SpanError> {
        match max {
            None => Self::at_least_checked(min),
            Some(max) => Self::between_checked(min, max),
        }
    }
}
