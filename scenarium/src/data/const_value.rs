use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;
use std::slice;

use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub enum ConstValue {
    #[default]
    Null,
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
    FsPath(String),
    FsPaths(Vec<String>),
    Enum(String),
}

impl PartialEq for ConstValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Float(left), Self::Float(right)) => left.to_bits() == right.to_bits(),
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            (Self::FsPath(left), Self::FsPath(right)) => left == right,
            (Self::FsPaths(left), Self::FsPaths(right)) => left == right,
            (Self::Enum(left), Self::Enum(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for ConstValue {}

impl ConstValue {
    /// The scalar coercion class, value side: the three kinds
    /// [`as_f64`](Self::as_f64)/[`as_i64`](Self::as_i64)/[`as_bool`](Self::as_bool)
    /// convert freely between. `DataType::is_numeric_scalar` is the same class
    /// on the type side, and
    /// [`DataType::accepts_const`](crate::DataType) reads this one to hold a
    /// literal to exactly what a runtime read of it would accept.
    pub(crate) const fn is_numeric_scalar(&self) -> bool {
        matches!(self, Self::Float(_) | Self::Int(_) | Self::Bool(_))
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            Self::Int(value) => Some(*value as f64),
            Self::Bool(value) => Some(i64::from(*value) as f64),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            Self::Float(value) => Some(*value as i64),
            Self::Bool(value) => Some(i64::from(*value)),
            _ => None,
        }
    }

    /// Scalar truthiness: **any** nonzero value reads as `true`, per the
    /// coercion class on [`crate::DataType::compatible_with`].
    ///
    /// The float arm compares against zero exactly, deliberately. A
    /// tolerance here is not float hygiene — there is no subtraction
    /// whose rounding it would absorb — it is a second, narrower
    /// definition of zero that silently reads small-but-real magnitudes
    /// (`f64::MIN_POSITIVE`, a normalized weight, a converged residual)
    /// as `false`. NaN is not zero and so reads as `true`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Int(value) => Some(*value != 0),
            Self::Float(value) => Some(*value != 0.0),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_enum(&self) -> Option<&str> {
        match self {
            Self::Enum(variant) => Some(variant),
            _ => None,
        }
    }

    pub fn as_fs_path(&self) -> Option<&str> {
        match self {
            Self::FsPath(path) => Some(path),
            _ => None,
        }
    }

    /// The paths this value names, one variant or many — a single path
    /// answers as a one-element slice, so a caller that works over a
    /// selection never has to special-case the singular form.
    pub fn as_fs_paths(&self) -> Option<&[String]> {
        match self {
            Self::FsPath(path) => Some(slice::from_ref(path)),
            Self::FsPaths(paths) => Some(paths),
            _ => None,
        }
    }

    /// The literal's canonical text, rendered on demand: unquoted and
    /// full-precision, the form an editor shows and parses back — as against
    /// [`Display`], which quotes strings and rounds floats for reading.
    ///
    /// A renderer rather than a `String` so a per-frame reader can write it
    /// into a buffer it already holds — `write!(buf, "{}", v.value_text())` —
    /// instead of taking one it would only copy out of and drop.
    pub const fn value_text(&self) -> ValueText<'_> {
        ValueText(self)
    }

    pub fn to_value_string(&self) -> String {
        self.value_text().to_string()
    }
}

/// [`ConstValue::value_text`]'s renderer.
#[derive(Clone, Copy, Debug)]
pub struct ValueText<'a>(&'a ConstValue);

impl Display for ValueText<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.0 {
            ConstValue::Null => f.write_str("null"),
            ConstValue::Float(value) => write!(f, "{value}"),
            ConstValue::Int(value) => write!(f, "{value}"),
            ConstValue::Bool(value) => write!(f, "{value}"),
            ConstValue::String(value) | ConstValue::FsPath(value) | ConstValue::Enum(value) => {
                f.write_str(value)
            }
            ConstValue::FsPaths(paths) => {
                // The `join("\n")` this replaces, without its intermediate.
                if let Some((first, rest)) = paths.split_first() {
                    f.write_str(first)?;
                    for path in rest {
                        write!(f, "\n{path}")?;
                    }
                }
                Ok(())
            }
        }
    }
}

impl Display for ConstValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "null"),
            Self::Float(value) => write!(f, "{value:.4}"),
            Self::Int(value) => write!(f, "{value}"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::String(value) => write!(f, "\"{value}\""),
            Self::FsPath(path) => write!(f, "\"{path}\""),
            Self::FsPaths(paths) => write!(f, "{} paths", paths.len()),
            Self::Enum(variant) => write!(f, "{variant}"),
        }
    }
}

impl From<i64> for ConstValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<i32> for ConstValue {
    fn from(value: i32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<f32> for ConstValue {
    fn from(value: f32) -> Self {
        Self::Float(f64::from(value))
    }
}

impl From<f64> for ConstValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for ConstValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<String> for ConstValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<bool> for ConstValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fmt::Write as _;

    #[test]
    fn coercions_cover_numeric_and_named_values() {
        assert_eq!(ConstValue::Int(3).as_f64(), Some(3.0));
        assert_eq!(ConstValue::Bool(true).as_f64(), Some(1.0));
        assert_eq!(ConstValue::Float(2.7).as_i64(), Some(2));
        assert_eq!(ConstValue::Float(0.0).as_bool(), Some(false));
        assert_eq!(ConstValue::Float(-0.0).as_bool(), Some(false));
        assert_eq!(ConstValue::Float(0.5).as_bool(), Some(true));
        // Any nonzero reads as true, however small. An epsilon band here
        // would read each of these as `false` despite being real values.
        for tiny in [f64::MIN_POSITIVE, f64::EPSILON, -f64::EPSILON, 1e-300] {
            assert_eq!(
                ConstValue::Float(tiny).as_bool(),
                Some(true),
                "nonzero float {tiny:e} must read as true",
            );
        }
        assert_eq!(ConstValue::Float(f64::NAN).as_bool(), Some(true));
        assert_eq!(ConstValue::String("x".into()).as_f64(), None);
        assert_eq!(ConstValue::String("hi".into()).as_string(), Some("hi"));
        assert_eq!(ConstValue::Enum("Add".into()).as_enum(), Some("Add"));
        // `as_fs_paths` spans both variants — a caller reading a selection
        // sees one path as a selection of one — while `as_fs_path` stays
        // exact about which variant it was handed.
        let one_path = ConstValue::FsPath("x".into());
        assert_eq!(one_path.as_fs_path(), Some("x"));
        assert_eq!(
            one_path.as_fs_paths(),
            Some(["x".to_string()].as_slice()),
            "a single path answers as a one-element selection"
        );
        let paths = ConstValue::FsPaths(vec!["x".into(), "y".into()]);
        assert_eq!(paths.as_fs_path(), None);
        assert_eq!(
            paths.as_fs_paths(),
            Some(["x".to_string(), "y".to_string()].as_slice())
        );
        assert_eq!(ConstValue::Int(7).as_fs_paths(), None);
    }

    #[test]
    fn value_string_and_display_have_distinct_formats() {
        assert_eq!(ConstValue::Float(1.5).to_value_string(), "1.5");
        assert_eq!(ConstValue::String("hi".into()).to_value_string(), "hi");
        assert_eq!(
            ConstValue::FsPath("a.fit".into()).to_value_string(),
            "a.fit"
        );
        assert_eq!(
            ConstValue::FsPaths(vec!["a.fit".into(), "b.fit".into()]).to_value_string(),
            "a.fit\nb.fit"
        );
        assert_eq!(ConstValue::Null.to_value_string(), "null");
        assert_eq!(ConstValue::Int(-7).to_value_string(), "-7");
        assert_eq!(ConstValue::Bool(true).to_value_string(), "true");
        assert_eq!(
            ConstValue::Enum("Linear".into()).to_value_string(),
            "Linear"
        );
        assert_eq!(ConstValue::FsPaths(Vec::new()).to_value_string(), "");
        // The renderer writes into a buffer that already holds something,
        // which is the whole reason it exists beside the owned spelling.
        let mut buf = String::from("> ");
        write!(buf, "{}", ConstValue::Int(7).value_text()).unwrap();
        assert_eq!(buf, "> 7");
        assert_eq!(format!("{}", ConstValue::Float(1.5)), "1.5000");
        assert_eq!(format!("{}", ConstValue::String("hi".into())), "\"hi\"");
        assert_eq!(
            format!("{}", ConstValue::FsPath("a.fit".into())),
            "\"a.fit\""
        );
        assert_eq!(
            format!(
                "{}",
                ConstValue::FsPaths(vec!["a.fit".into(), "b.fit".into()])
            ),
            "2 paths"
        );
    }
}
