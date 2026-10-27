//! [`ShownValue`]: an input's constant as the editor shows it, on the node and in the inspector.

use std::fmt;
use std::fmt::{Display, Formatter};

use scenarium::{ConstValue, DataType};

use crate::gui::widgets::format::path_preview::PathPreview;

/// What an input with nothing in it reads, on the node and in the inspector.
pub(crate) const UNSET: &str = "\u{2014}";

/// The decimals a float shows: the node's field writes them and the inspector reads them, so
/// one value never shows two ways.
pub(crate) const FLOAT_DECIMALS: usize = 3;

/// A constant of an input of `data_type`, as the node's field shows it: a float at
/// [`FLOAT_DECIMALS`], a path by its file name, several by their count, nothing as [`UNSET`];
/// on an untyped (`Any`) input, as its text field does, [`write_untyped`]. Rendered on demand,
/// so a per-frame label builds no `String`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ShownValue<'a> {
    pub(crate) value: &'a ConstValue,
    pub(crate) data_type: &'a DataType,
}

impl Display for ShownValue<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if *self.data_type == DataType::Any {
            return match self.value {
                ConstValue::Null => f.write_str(UNSET),
                value => write_untyped(value, f),
            };
        }
        match self.value {
            ConstValue::Null => f.write_str(UNSET),
            ConstValue::Float(value) => write!(f, "{value:.FLOAT_DECIMALS$}"),
            ConstValue::Int(value) => write!(f, "{value}"),
            ConstValue::Bool(value) => write!(f, "{value}"),
            ConstValue::String(value) | ConstValue::Enum(value) => f.write_str(value),
            ConstValue::FsPath(path) => {
                let DataType::FsPath(config) = self.data_type else {
                    unreachable!("a path literal is admitted only on a path or an untyped port");
                };
                PathPreview::single(path, config.mode).fmt(f)
            }
            ConstValue::FsPaths(paths) => PathPreview::multi(paths).fmt(f),
        }
    }
}

/// An untyped input's constant as its text field shows it and parses back: a float with its
/// `.0` (`{:?}`), as `{}` prints `1` for a whole one, which reads to a user as an integer; any
/// other kind as its own text; nothing blank.
pub(crate) fn write_untyped(value: &ConstValue, out: &mut impl fmt::Write) -> fmt::Result {
    match value {
        ConstValue::Null => Ok(()),
        ConstValue::Float(value) => write!(out, "{value:?}"),
        other => write!(out, "{}", other.value_text()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use scenarium::{ConstValue, DataType, FsPathConfig};

    use crate::gui::widgets::format::shown_value::ShownValue;

    /// Each kind as the node's field shows it: a float at three decimals, a path by its file
    /// name, several by their count, nothing as a dash; on an untyped input, as its text field
    /// writes it, a whole float keeping its `.0` and a path whole.
    #[test]
    fn each_constant_reads_as_the_node_shows_it() {
        let path = DataType::FsPath(Arc::new(FsPathConfig::default()));
        let cases: [(ConstValue, &DataType, &str); 11] = [
            (ConstValue::Float(5.0), &DataType::Float, "5.000"),
            (ConstValue::Float(0.15), &DataType::Float, "0.150"),
            (ConstValue::Int(-3), &DataType::Int, "-3"),
            (ConstValue::Bool(true), &DataType::Bool, "true"),
            (ConstValue::String("a b".into()), &DataType::String, "a b"),
            (
                ConstValue::FsPath("frames/light-01.raf".into()),
                &path,
                "light-01.raf",
            ),
            (
                ConstValue::FsPaths(vec!["a.raf".into(), "b.raf".into()]),
                &path,
                "2 files",
            ),
            (ConstValue::Null, &DataType::Float, "\u{2014}"),
            (ConstValue::Float(1.0), &DataType::Any, "1.0"),
            (
                ConstValue::FsPath("frames/a.raf".into()),
                &DataType::Any,
                "frames/a.raf",
            ),
            (ConstValue::Null, &DataType::Any, "\u{2014}"),
        ];
        for (value, data_type, shown) in cases {
            let text = ShownValue {
                value: &value,
                data_type,
            }
            .to_string();
            assert_eq!(text, shown, "{value:?}");
        }
    }
}
