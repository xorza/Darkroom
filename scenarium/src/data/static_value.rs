use std::fmt::Display;

use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub enum StaticValue {
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

impl PartialEq for StaticValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (StaticValue::Null, StaticValue::Null) => true,
            (StaticValue::Float(left), StaticValue::Float(right)) => {
                left.to_bits() == right.to_bits()
            }
            (StaticValue::Int(left), StaticValue::Int(right)) => left == right,
            (StaticValue::Bool(left), StaticValue::Bool(right)) => left == right,
            (StaticValue::String(left), StaticValue::String(right)) => left == right,
            (StaticValue::FsPath(left), StaticValue::FsPath(right)) => left == right,
            (StaticValue::FsPaths(left), StaticValue::FsPaths(right)) => left == right,
            (StaticValue::Enum(left), StaticValue::Enum(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for StaticValue {}

impl StaticValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            StaticValue::Float(value) => Some(*value),
            StaticValue::Int(value) => Some(*value as f64),
            StaticValue::Bool(value) => Some(*value as i64 as f64),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            StaticValue::Int(value) => Some(*value),
            StaticValue::Float(value) => Some(*value as i64),
            StaticValue::Bool(value) => Some(*value as i64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            StaticValue::Bool(value) => Some(*value),
            StaticValue::Int(value) => Some(*value != 0),
            StaticValue::Float(value) => Some(value.abs() > f64::EPSILON),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            StaticValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_enum(&self) -> Option<&str> {
        match self {
            StaticValue::Enum(variant) => Some(variant),
            _ => None,
        }
    }

    pub fn as_fs_path(&self) -> Option<&str> {
        match self {
            StaticValue::FsPath(path) => Some(path),
            _ => None,
        }
    }

    pub fn as_fs_paths(&self) -> Option<&[String]> {
        match self {
            StaticValue::FsPaths(paths) => Some(paths),
            _ => None,
        }
    }

    pub fn to_value_string(&self) -> String {
        match self {
            StaticValue::Null => "null".to_string(),
            StaticValue::Float(value) => value.to_string(),
            StaticValue::Int(value) => value.to_string(),
            StaticValue::Bool(value) => value.to_string(),
            StaticValue::String(value) => value.clone(),
            StaticValue::FsPath(path) => path.clone(),
            StaticValue::FsPaths(paths) => paths.join("\n"),
            StaticValue::Enum(variant) => variant.clone(),
        }
    }
}

impl Display for StaticValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StaticValue::Null => write!(f, "null"),
            StaticValue::Float(value) => write!(f, "{value:.4}"),
            StaticValue::Int(value) => write!(f, "{value}"),
            StaticValue::Bool(value) => write!(f, "{value}"),
            StaticValue::String(value) => write!(f, "\"{value}\""),
            StaticValue::FsPath(path) => write!(f, "\"{path}\""),
            StaticValue::FsPaths(paths) => write!(f, "{} paths", paths.len()),
            StaticValue::Enum(variant) => write!(f, "{variant}"),
        }
    }
}

impl From<i64> for StaticValue {
    fn from(value: i64) -> Self {
        StaticValue::Int(value)
    }
}

impl From<i32> for StaticValue {
    fn from(value: i32) -> Self {
        StaticValue::Int(value as i64)
    }
}

impl From<f32> for StaticValue {
    fn from(value: f32) -> Self {
        StaticValue::Float(value as f64)
    }
}

impl From<f64> for StaticValue {
    fn from(value: f64) -> Self {
        StaticValue::Float(value)
    }
}

impl From<&str> for StaticValue {
    fn from(value: &str) -> Self {
        StaticValue::String(value.to_string())
    }
}

impl From<String> for StaticValue {
    fn from(value: String) -> Self {
        StaticValue::String(value)
    }
}

impl From<bool> for StaticValue {
    fn from(value: bool) -> Self {
        StaticValue::Bool(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coercions_cover_numeric_and_named_values() {
        assert_eq!(StaticValue::Int(3).as_f64(), Some(3.0));
        assert_eq!(StaticValue::Bool(true).as_f64(), Some(1.0));
        assert_eq!(StaticValue::Float(2.7).as_i64(), Some(2));
        assert_eq!(StaticValue::Float(0.0).as_bool(), Some(false));
        assert_eq!(StaticValue::Float(0.5).as_bool(), Some(true));
        assert_eq!(StaticValue::String("x".into()).as_f64(), None);
        assert_eq!(StaticValue::String("hi".into()).as_string(), Some("hi"));
        assert_eq!(StaticValue::Enum("Add".into()).as_enum(), Some("Add"));
        let one_path = StaticValue::FsPath("x".into());
        assert_eq!(one_path.as_fs_path(), Some("x"));
        assert_eq!(one_path.as_fs_paths(), None);
        let paths = StaticValue::FsPaths(vec!["x".into(), "y".into()]);
        assert_eq!(paths.as_fs_path(), None);
        assert_eq!(
            paths.as_fs_paths(),
            Some(["x".to_string(), "y".to_string()].as_slice())
        );
    }

    #[test]
    fn value_string_and_display_have_distinct_formats() {
        assert_eq!(StaticValue::Float(1.5).to_value_string(), "1.5");
        assert_eq!(StaticValue::String("hi".into()).to_value_string(), "hi");
        assert_eq!(
            StaticValue::FsPath("a.fit".into()).to_value_string(),
            "a.fit"
        );
        assert_eq!(
            StaticValue::FsPaths(vec!["a.fit".into(), "b.fit".into()]).to_value_string(),
            "a.fit\nb.fit"
        );
        assert_eq!(format!("{}", StaticValue::Float(1.5)), "1.5000");
        assert_eq!(format!("{}", StaticValue::String("hi".into())), "\"hi\"");
        assert_eq!(
            format!("{}", StaticValue::FsPath("a.fit".into())),
            "\"a.fit\""
        );
        assert_eq!(
            format!(
                "{}",
                StaticValue::FsPaths(vec!["a.fit".into(), "b.fit".into()])
            ),
            "2 paths"
        );
    }
}
