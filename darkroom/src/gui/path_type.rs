//! [`PathType`]: a port type that names a path, as its pick button and dialog take it.

use std::fmt;
use std::fmt::{Display, Formatter};

use scenarium::{DataType, FileKind, FileKindId, FileMode, GraphSet, Library};

/// A port type that names a path: a directory, or a file of a registered kind taken as its mode
/// says. What the pick button reads and which dialog it opens, parsed once off a [`DataType`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathType {
    Dir,
    File { kind: FileKindId, mode: FileMode },
}

impl PathType {
    /// The path type `data_type` is, or `None` for a type that names no path.
    pub(crate) const fn of(data_type: DataType) -> Option<Self> {
        match data_type {
            DataType::Dir => Some(Self::Dir),
            DataType::File { kind, mode } => Some(Self::File { kind, mode }),
            DataType::Any
            | DataType::Float
            | DataType::Int
            | DataType::Bool
            | DataType::String
            | DataType::Custom(_)
            | DataType::Enum(_) => None,
        }
    }

    /// What a pick button reads while nothing is chosen.
    pub(crate) const fn prompt(self) -> &'static str {
        match self {
            Self::Dir => "Choose directory…",
            Self::File {
                mode: FileMode::Open,
                ..
            } => "Choose file…",
            Self::File {
                mode: FileMode::OpenMany,
                ..
            } => "Choose files…",
            Self::File {
                mode: FileMode::Save,
                ..
            } => "Choose save path…",
        }
    }

    /// The extensions a dialog filters by: the file kind's, or none, which lists every file, for
    /// a directory and for a kind `library` does not register.
    pub(crate) fn extensions(self, library: &Library) -> &[String] {
        match self {
            Self::Dir => &[],
            Self::File { kind, .. } => library.file_kind(kind).map_or(&[], FileKind::extensions),
        }
    }

    /// What a port's tooltip says of the type: the kind and how the port takes it, with the
    /// extensions it lists.
    pub(crate) const fn label<'a>(
        self,
        graphs: &'a GraphSet,
        library: &'a Library,
    ) -> PathLabel<'a> {
        PathLabel {
            path_type: self,
            graphs,
            library,
        }
    }
}

/// A path type as a tooltip names it, rendered on demand: `directory`, or the kind's name, how
/// the port takes it and the extensions its dialog lists, as `Image · file (png, tif)`.
#[derive(Debug)]
pub(crate) struct PathLabel<'a> {
    path_type: PathType,
    graphs: &'a GraphSet,
    library: &'a Library,
}

impl Display for PathLabel<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let PathType::File { kind, mode } = self.path_type else {
            return f.write_str("directory");
        };
        let how = match mode {
            FileMode::Open => "file",
            FileMode::OpenMany => "files",
            FileMode::Save => "save path",
        };
        let name = self
            .graphs
            .type_name(self.library, DataType::File { kind, mode });
        write!(f, "{name} · {how}")?;
        // Written one at a time, not `join`ed, which would build the intermediate string.
        if let Some((first, rest)) = self.path_type.extensions(self.library).split_first() {
            write!(f, " ({first}")?;
            for extension in rest {
                write!(f, ", {extension}")?;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{DataType, FileKind, FileKindId, FileMode, Library};

    use crate::gui::path_type::PathType;

    /// Only a directory and a file name a path; each reads its own prompt.
    #[test]
    fn a_path_type_is_a_directory_or_a_file_and_prompts_for_its_mode() {
        let kind = FileKindId::from_u128(1);
        let file = |mode| DataType::File { kind, mode };
        assert_eq!(PathType::of(DataType::String), None);
        assert_eq!(PathType::of(DataType::Any), None);
        for (data_type, prompt) in [
            (DataType::Dir, "Choose directory…"),
            (file(FileMode::Open), "Choose file…"),
            (file(FileMode::OpenMany), "Choose files…"),
            (file(FileMode::Save), "Choose save path…"),
        ] {
            assert_eq!(
                PathType::of(data_type).unwrap().prompt(),
                prompt,
                "{data_type:?}"
            );
        }
    }

    /// A dialog lists a registered kind's extensions, and none, which is every file, for a
    /// directory and for a kind the library does not register.
    #[test]
    fn a_dialog_filters_by_the_registered_kinds_extensions() {
        let [image, unknown] = [1, 2].map(FileKindId::from_u128);
        let mut library = Library::default();
        library.register_file_kind(image, FileKind::new("Image", ["png", "tif"]));
        let open = |kind| PathType::File {
            kind,
            mode: FileMode::Open,
        };
        assert_eq!(open(image).extensions(&library), ["png", "tif"]);
        assert!(open(unknown).extensions(&library).is_empty());
        assert!(PathType::Dir.extensions(&library).is_empty());
    }

    /// A tooltip names the kind, how the port takes it, and the extensions its dialog lists,
    /// the separator only *between* them; a kind with no extensions, and one the library does
    /// not register, list none.
    #[test]
    fn a_label_names_the_kind_its_mode_and_extensions() {
        let [image, any, unknown] = [1, 2, 3].map(FileKindId::from_u128);
        let mut library = Library::default();
        library.register_file_kind(image, FileKind::new("Image", ["fit", "fits", "raf"]));
        library.register_file_kind(any, FileKind::new("File", Vec::<&str>::new()));
        let graphs = scenarium::GraphSet::default();
        let label = |data_type| {
            PathType::of(data_type)
                .unwrap()
                .label(&graphs, &library)
                .to_string()
        };
        let file = |kind, mode| DataType::File { kind, mode };
        assert_eq!(label(DataType::Dir), "directory");
        assert_eq!(
            label(file(image, FileMode::OpenMany)),
            "Image · files (fit, fits, raf)"
        );
        assert_eq!(label(file(any, FileMode::Open)), "File · file");
        assert_eq!(
            label(file(unknown, FileMode::Save)),
            "unknown type · save path"
        );
    }
}
