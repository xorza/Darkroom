//! [`FileKind`]: a registered set of file extensions under a name, as "Image" or "Raw".
//!
//! # Decisions
//!
//! - **The kind is registered, not the whole file type.** "Image" is one entry for the port that
//!   opens one, the port that opens several and the port that saves, each of which names the
//!   kind with its own [`FileMode`](crate::FileMode). As Apple's type identifiers, each a
//!   registered id with its extensions
//!   ([uniform type identifiers](https://developer.apple.com/documentation/uniformtypeidentifiers)).
//! - **One kind is within another when every file it holds, the other holds**, so a "PNG"
//!   output feeds an "Image" input and not the reverse. Apple and the freedesktop database
//!   declare this relation by hand, `public.png` conforming to `public.image` and `sub-class-of`
//!   ([shared MIME-info](https://specifications.freedesktop.org/shared-mime-info/0.18/ar01s02.html)),
//!   as their types are told by content; a kind here is its list of extensions, so the relation
//!   is computed and cannot drift.

use common::Id;

/// The id of a file kind a [`DataType::File`](crate::DataType::File) names.
pub type FileKindId = Id<FileKind>;

/// A set of file extensions under a name. No extensions is every file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileKind {
    name: String,
    /// Lowercase, without the dot, each once, in the order a dialog lists them.
    extensions: Vec<String>,
}

impl FileKind {
    /// A kind named `name` of the files with `extensions`; none is every file.
    ///
    /// # Panics
    /// If an extension is empty, holds a dot or an uppercase letter, or is listed twice: a
    /// registration a programmer wrote, held to one spelling so two kinds compare by their lists.
    pub fn new(
        name: impl Into<String>,
        extensions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::of(
            name.into(),
            extensions.into_iter().map(Into::into).collect(),
        )
    }

    fn of(name: String, extensions: Vec<String>) -> Self {
        for (idx, extension) in extensions.iter().enumerate() {
            assert!(!extension.is_empty(), "a file extension is not empty");
            assert!(
                !extension.contains('.'),
                "a file extension is written without its dot: {extension:?}"
            );
            assert!(
                !extension.chars().any(char::is_uppercase),
                "a file extension is written in lowercase: {extension:?}"
            );
            assert!(
                !extensions[..idx].contains(extension),
                "the file extension {extension:?} is listed twice"
            );
        }
        Self { name, extensions }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The extensions, lowercase and without the dot; empty for every file.
    pub fn extensions(&self) -> &[String] {
        &self.extensions
    }

    /// Whether every file this kind holds, `other` holds too.
    pub fn within(&self, other: &Self) -> bool {
        other.extensions.is_empty()
            || (!self.extensions.is_empty()
                && self
                    .extensions
                    .iter()
                    .all(|extension| other.extensions.contains(extension)))
    }
}
