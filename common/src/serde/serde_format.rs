/// The encoding of a serialized value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SerdeFormat {
    /// Pretty-printed RON text, for files a person reads or edits.
    Ron,
    /// Compact binary, for values only the program reads back.
    Bitcode,
}

#[cfg(any(test, feature = "internals"))]
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
