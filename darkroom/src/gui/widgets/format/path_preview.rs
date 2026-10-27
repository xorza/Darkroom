//! [`PathPreview`]: what a path input reads, on a node's pick button and in the inspector.

use std::borrow::Cow;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::path::Path;

use scenarium::FsPathMode;

/// What an `FsPath` input's pick button reads: the mode's call to action
/// while nothing is chosen, a single chosen path's own file name, or how many
/// files a multi-pick holds.
///
/// Rendered on demand rather than assembled into a `String`: the button
/// records every frame the node does, and the label goes straight into the
/// record pass's text arena.
#[derive(Debug)]
pub(crate) enum PathPreview<'a> {
    /// Nothing chosen — the prompt for the mode being picked for.
    Prompt(&'static str),
    /// The chosen path's final component (the whole path when it has none).
    Name(Cow<'a, str>),
    /// How many non-empty paths a multi-pick holds. Never zero — that reads
    /// as a [`Prompt`](Self::Prompt).
    Count(usize),
}

impl<'a> PathPreview<'a> {
    /// The label for a single-path literal.
    pub(crate) fn single(path: &'a str, mode: FsPathMode) -> Self {
        if path.is_empty() {
            return Self::Prompt(match mode {
                FsPathMode::ExistingFile => "Choose file…",
                FsPathMode::ExistingFiles => "Choose files…",
                FsPathMode::NewFile => "Choose save path…",
                FsPathMode::Dir => "Choose directory…",
            });
        }
        Self::Name(
            Path::new(path)
                .file_name()
                .map_or(Cow::Borrowed(path), |name| name.to_string_lossy()),
        )
    }

    /// The label for a multi-path literal: a count, since no one file name
    /// stands for the selection.
    pub(crate) fn multi(paths: &[String]) -> Self {
        match paths.iter().filter(|path| !path.is_empty()).count() {
            0 => Self::Prompt("Choose files…"),
            count => Self::Count(count),
        }
    }
}

impl Display for PathPreview<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prompt(prompt) => f.write_str(prompt),
            Self::Name(name) => f.write_str(name),
            Self::Count(1) => f.write_str("1 file"),
            Self::Count(count) => write!(f, "{count} files"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use scenarium::FsPathMode;

    use crate::gui::widgets::format::path_preview::PathPreview;

    #[test]
    fn path_previews_distinguish_single_modes_and_multi_selections() {
        let single = |path, mode| PathPreview::single(path, mode).to_string();
        let multi = |paths: &[String]| PathPreview::multi(paths).to_string();

        assert_eq!(single("", FsPathMode::ExistingFile), "Choose file…");
        assert_eq!(single("", FsPathMode::NewFile), "Choose save path…");
        assert_eq!(single("", FsPathMode::Dir), "Choose directory…");
        assert_eq!(
            single("frames/light-01.raf", FsPathMode::ExistingFile),
            "light-01.raf"
        );
        // The name is a slice of the path it came from, not a rebuild of it —
        // which is what keeps a per-frame button label off the heap.
        assert!(matches!(
            PathPreview::single("frames/light-01.raf", FsPathMode::ExistingFile),
            PathPreview::Name(Cow::Borrowed("light-01.raf"))
        ));

        assert_eq!(multi(&[]), "Choose files…");
        assert_eq!(multi(&[String::new()]), "Choose files…");
        assert_eq!(multi(&["frames/light-01.raf".to_string()]), "1 file");
        assert_eq!(
            multi(&["a.raf".to_string(), "b.raf".to_string()]),
            "2 files"
        );
    }
}
