use std::fmt;

use ron::de;
use ron::ser;
use ron::ser::PrettyConfig;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::serde::{DeserializeError, SerializeError};

/// The encoding of a serialized value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SerdeFormat {
    /// Pretty-printed RON text, for files a person reads or edits. Lines end in `\n` on every
    /// system, so one value is the same bytes wherever it is written: ron's default is `\r\n` on
    /// Windows.
    Ron,
    /// Compact binary, for values only the program reads back.
    Bitcode,
}

impl SerdeFormat {
    pub fn serialize<T: Serialize + ?Sized>(self, value: &T) -> Result<Vec<u8>, SerializeError> {
        let mut bytes = Vec::new();
        self.serialize_into(value, &mut bytes)?;
        Ok(bytes)
    }

    /// Appends `value`'s encoding to `out`; on an error `out` may hold part of it.
    ///
    /// RON text goes straight into `out`. Bitcode's serde encoder builds its column buffers and
    /// the finished bytes for each call, so that arm allocates whatever `out` holds.
    pub fn serialize_into<T: Serialize + ?Sized>(
        self,
        value: &T,
        out: &mut Vec<u8>,
    ) -> Result<(), SerializeError> {
        match self {
            Self::Ron => ser::to_writer_pretty(
                Utf8Writer(out),
                value,
                PrettyConfig::default().new_line("\n"),
            )?,
            Self::Bitcode => out.extend_from_slice(&bitcode::serialize(value)?),
        }
        Ok(())
    }

    pub fn deserialize<T: DeserializeOwned>(
        self,
        serialized: &[u8],
    ) -> Result<T, DeserializeError> {
        match self {
            Self::Ron => Ok(de::from_bytes(serialized)?),
            Self::Bitcode => Ok(bitcode::deserialize(serialized)?),
        }
    }
}

/// [`fmt::Write`] over a byte buffer.
///
/// RON's serializer writes text through [`fmt::Write`], and the caller's buffer is where that
/// text goes, so the bridge appends to it with no `String` between. Writing never fails, so no
/// error is lost by the bridge.
#[derive(Debug)]
struct Utf8Writer<'a>(&'a mut Vec<u8>);

impl fmt::Write for Utf8Writer<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::serde::serde_format::SerdeFormat;

    impl SerdeFormat {
        /// Every format, for a test that runs over all of them. The match
        /// below stops compiling on a new variant, which is the reminder to
        /// list it here.
        pub const ALL: [Self; 2] = [Self::Ron, Self::Bitcode];
    }

    const _: () = match SerdeFormat::Ron {
        SerdeFormat::Ron | SerdeFormat::Bitcode => (),
    };
}
