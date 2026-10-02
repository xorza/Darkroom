/// The encoding of a serialized value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SerdeFormat {
    /// Pretty-printed RON text, for files a person reads or edits.
    Ron,
    /// Compact binary, for values only the program reads back.
    Bitcode,
}
