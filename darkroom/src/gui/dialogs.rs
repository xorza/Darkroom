//! [`Dialogs`]: the file pickers `App` asks for paths through — native ones (rfd) in the window,
//! scripted ones in the harness. Project byte⇄type plumbing lives in `crate::core::io::document`;
//! this side hands paths off to that GUI-free module. Failures degrade — a cancelled or failed
//! pick returns `None` — rather than crashing. Everything else the shell does for us is in
//! `crate::platform`.

#[cfg(feature = "internals")]
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use common::fs;

use crate::core::io::document;
#[cfg(feature = "internals")]
use crate::internals::harness::action::DialogAnswer;

/// Where `App`'s path picks are answered.
#[derive(Debug)]
pub(crate) enum Dialogs {
    /// The platform's file dialogs, which block until the user answers.
    Native,
    /// Answers given ahead, one per ask in order: a session no one can see must not block on a
    /// dialog. An ask with no answer left, or with an answer of the other shape, reads as a
    /// cancel and logs an error, which the harness reports.
    #[cfg(feature = "internals")]
    Scripted(VecDeque<DialogAnswer>),
}

/// A kind of file a save names: the label of its dialog's filter, and the extension the save
/// adds when the typed name lacks it, as the native dialogs do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SavedFile {
    label: &'static str,
    extension: &'static str,
}

impl SavedFile {
    pub(crate) const PROJECT: Self = Self {
        label: "Darkroom project",
        extension: document::EXTENSION,
    };
    pub(crate) const PNG: Self = Self {
        label: "PNG image",
        extension: "png",
    };

    /// `path` with this kind's extension, which replaces any other; one that matches without
    /// regard to case stays as typed.
    fn extended(self, mut path: PathBuf) -> PathBuf {
        let has = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(self.extension));
        if !has {
            path.set_extension(self.extension);
        }
        path
    }

    /// The file a save to `picked` would replace without the dialog having asked about it: the
    /// extended name, when it differs from the picked one and exists.
    fn unconfirmed_target(self, picked: &Path) -> Option<PathBuf> {
        let target = self.extended(picked.to_path_buf());
        (target != picked && fs::exists(&target)).then_some(target)
    }
}

#[cfg_attr(
    not(feature = "internals"),
    expect(
        clippy::needless_pass_by_ref_mut,
        reason = "the scripted answers, under `internals`, are what a pick mutates"
    )
)]
impl Dialogs {
    /// A project file to open, the picker starting beside `start`.
    pub(crate) fn project_open_path(&mut self, start: Option<&Path>) -> Option<PathBuf> {
        match self {
            Self::Native => pick_project_open_path(start),
            #[cfg(feature = "internals")]
            Self::Scripted(answers) => scripted_path(answers, "open a project"),
        }
    }

    /// Where to save a file of `kind`, the picker starting beside `start`; the extension is added.
    pub(crate) fn save_path(&mut self, kind: SavedFile, start: Option<&Path>) -> Option<PathBuf> {
        match self {
            Self::Native => pick_save_path(kind, start),
            #[cfg(feature = "internals")]
            Self::Scripted(answers) => scripted_path(answers, &format!("save a {}", kind.label))
                .map(|path| kind.extended(path)),
        }
    }

    /// One existing file with one of `extensions`, or any file for none.
    pub(crate) fn existing_file(&mut self, extensions: &[&str]) -> Option<PathBuf> {
        match self {
            Self::Native => pick_existing_file(extensions),
            #[cfg(feature = "internals")]
            Self::Scripted(answers) => scripted_path(answers, "pick a file"),
        }
    }

    /// Existing files with one of `extensions`, sorted, each once, at least one.
    pub(crate) fn existing_files(&mut self, extensions: &[&str]) -> Option<Vec<PathBuf>> {
        match self {
            Self::Native => pick_existing_files(extensions),
            #[cfg(feature = "internals")]
            Self::Scripted(answers) => match answers.pop_front() {
                Some(DialogAnswer::Paths(paths)) => {
                    normalize_file_selection(paths.into_iter().map(absolute).collect())
                }
                Some(DialogAnswer::Cancel) => None,
                answer => unanswered("pick files", answer.as_ref()),
            },
        }
    }

    /// A file to create, with one of `extensions`.
    pub(crate) fn new_file(&mut self, extensions: &[&str]) -> Option<PathBuf> {
        match self {
            Self::Native => pick_new_file(extensions),
            #[cfg(feature = "internals")]
            Self::Scripted(answers) => scripted_path(answers, "name a new file"),
        }
    }

    /// A directory.
    pub(crate) fn dir(&mut self) -> Option<PathBuf> {
        match self {
            Self::Native => pick_dir(),
            #[cfg(feature = "internals")]
            Self::Scripted(answers) => scripted_path(answers, "pick a folder"),
        }
    }
}

/// The next scripted answer to a one-path ask.
#[cfg(feature = "internals")]
fn scripted_path(answers: &mut VecDeque<DialogAnswer>, ask: &str) -> Option<PathBuf> {
    match answers.pop_front() {
        Some(DialogAnswer::Path(path)) => Some(absolute(path)),
        Some(DialogAnswer::Cancel) => None,
        answer => unanswered(ask, answer.as_ref()),
    }
}

/// `path` as a native dialog answers it: absolute, a relative one taken against the working
/// directory, so a scripted pick holds what a document keeps in memory.
#[cfg(feature = "internals")]
fn absolute(path: PathBuf) -> PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

/// A scripted ask with no answer of its shape: a cancel, and an error for the harness to report.
#[cfg(feature = "internals")]
fn unanswered<T>(ask: &str, answer: Option<&DialogAnswer>) -> Option<T> {
    match answer {
        None => tracing::error!("a dialog asked to {ask}, and no scripted answer was left"),
        Some(answer) => tracing::error!("a dialog asked to {ask}, and the answer was {answer:?}"),
    }
    None
}

fn file_dialog(start: Option<&Path>) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new();
    if let Some(parent) = start.and_then(Path::parent) {
        dialog = dialog.set_directory(parent);
    }
    dialog
}

fn pick_project_open_path(start: Option<&Path>) -> Option<PathBuf> {
    file_dialog(start)
        .add_filter("Darkroom project", &[document::EXTENSION])
        .pick_file()
}

/// The native dialog confirms overwriting the name the user typed, not the one
/// [`SavedFile::extended`] makes of it — so when that one already exists,
/// the dialog asks again with it filled in, and its own overwrite prompt
/// answers for the file that will be replaced. A second save dialog rather than
/// a message box, because rfd shows message boxes on Linux through zenity,
/// which a desktop may lack, and the save then fails with nothing on screen.
fn pick_save_path(kind: SavedFile, start: Option<&Path>) -> Option<PathBuf> {
    let mut picked = save_dialog(kind, start).save_file()?;
    while let Some(target) = kind.unconfirmed_target(&picked) {
        let mut dialog = save_dialog(kind, Some(&target));
        if let Some(name) = target.file_name().and_then(|name| name.to_str()) {
            dialog = dialog.set_file_name(name);
        }
        picked = dialog.save_file()?;
    }
    Some(kind.extended(picked))
}

fn save_dialog(kind: SavedFile, start: Option<&Path>) -> rfd::FileDialog {
    file_dialog(start).add_filter(kind.label, &[kind.extension])
}

fn filtered_file_dialog(extensions: &[&str]) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new();
    if !extensions.is_empty() {
        dialog = dialog.add_filter("Files", extensions);
    }
    dialog
}

fn pick_existing_file(extensions: &[&str]) -> Option<PathBuf> {
    filtered_file_dialog(extensions).pick_file()
}

fn pick_existing_files(extensions: &[&str]) -> Option<Vec<PathBuf>> {
    normalize_file_selection(filtered_file_dialog(extensions).pick_files()?)
}

fn normalize_file_selection(mut paths: Vec<PathBuf>) -> Option<Vec<PathBuf>> {
    paths.sort();
    paths.dedup();
    if paths.is_empty() { None } else { Some(paths) }
}

fn pick_new_file(extensions: &[&str]) -> Option<PathBuf> {
    filtered_file_dialog(extensions).save_file()
}

fn pick_dir() -> Option<PathBuf> {
    rfd::FileDialog::new().pick_folder()
}

#[cfg(test)]
mod tests {
    use std::path::{self, PathBuf};

    use common::{TempDir, fs};

    use std::collections::VecDeque;

    use crate::gui::dialogs::{DialogAnswer, Dialogs, SavedFile, normalize_file_selection};

    /// A save adds its kind's extension to a bare name and puts it in place of another, and keeps
    /// one that matches without regard to case. Only an extended name that exists and differs
    /// from the pick is owed a second confirmation: a bare name over nothing, and a pick that
    /// already carries the extension (the dialog asked about that very file), are not.
    #[test]
    fn only_an_existing_extended_name_needs_confirming_again() {
        let extended = |kind: SavedFile, name: &str| kind.extended(PathBuf::from(name));
        assert_eq!(
            extended(SavedFile::PROJECT, "scene"),
            PathBuf::from("scene.darkroom")
        );
        assert_eq!(
            extended(SavedFile::PROJECT, "scene.json"),
            PathBuf::from("scene.darkroom")
        );
        assert_eq!(
            extended(SavedFile::PROJECT, "scene.DARKROOM"),
            PathBuf::from("scene.DARKROOM")
        );
        assert_eq!(extended(SavedFile::PNG, "view"), PathBuf::from("view.png"));
        assert_eq!(
            extended(SavedFile::PNG, "view.PNG"),
            PathBuf::from("view.PNG")
        );

        let project = SavedFile::PROJECT;
        let dir = TempDir::new("darkroom-save-target");
        let bare = dir.join("scene");
        let extended = dir.join("scene.darkroom");
        assert_eq!(
            project.unconfirmed_target(&bare),
            None,
            "nothing to replace yet"
        );

        fs::write(&extended, b"x").unwrap();
        assert_eq!(project.unconfirmed_target(&bare), Some(extended.clone()));
        assert_eq!(
            SavedFile::PNG.unconfirmed_target(&bare),
            None,
            "another kind's file is not the one a save replaces"
        );
        assert_eq!(
            project.unconfirmed_target(&extended),
            None,
            "the dialog confirmed this file itself"
        );
        assert_eq!(
            project.unconfirmed_target(&dir.join("scene.DARKROOM")),
            None,
            "the extension matches without regard to case, so nothing is appended"
        );
    }

    #[test]
    fn file_selection_is_sorted_deduplicated_and_nonempty() {
        assert_eq!(normalize_file_selection(Vec::new()), None);
        assert_eq!(
            normalize_file_selection(vec![
                PathBuf::from("b.fit"),
                PathBuf::from("a.fit"),
                PathBuf::from("b.fit"),
            ]),
            Some(vec![PathBuf::from("a.fit"), PathBuf::from("b.fit")])
        );
    }

    /// Scripted answers go to the asks in order, each of its own shape and absolute, as a native
    /// dialog's are: a relative one is taken against the working directory. The project save adds
    /// the extension, as the native one does; the file list comes back sorted and once each. A
    /// cancel, an answer of the other shape, and an ask past the last answer all read as `None`.
    #[test]
    fn scripted_answers_go_to_the_asks_in_order() {
        // As a native dialog answers: absolute, so `/p` on Windows takes the working drive.
        let abs = |path: &str| path::absolute(path).unwrap();
        let mut dialogs = Dialogs::Scripted(VecDeque::from([
            DialogAnswer::Path(PathBuf::from("/p/scene.darkroom")),
            DialogAnswer::Path(PathBuf::from("/p/copy")),
            DialogAnswer::Paths(vec![PathBuf::from("/d/b.fit"), PathBuf::from("/d/a.fit")]),
            DialogAnswer::Cancel,
            DialogAnswer::Paths(vec![PathBuf::from("/d/c.fit")]),
            DialogAnswer::Path(PathBuf::from("/d")),
            DialogAnswer::Path(PathBuf::from("inputs/x.fit")),
        ]));
        assert_eq!(
            dialogs.project_open_path(None),
            Some(abs("/p/scene.darkroom"))
        );
        assert_eq!(
            dialogs.save_path(SavedFile::PROJECT, None),
            Some(abs("/p/copy.darkroom"))
        );
        assert_eq!(
            dialogs.existing_files(&["fit"]),
            Some(vec![abs("/d/a.fit"), abs("/d/b.fit")])
        );
        assert_eq!(dialogs.new_file(&[]), None, "cancelled");
        assert_eq!(
            dialogs.existing_file(&["fit"]),
            None,
            "a list for a one-file ask"
        );
        assert_eq!(dialogs.dir(), Some(abs("/d")));
        let relative = dialogs.existing_file(&["fit"]).unwrap();
        assert!(relative.is_absolute(), "{}", relative.display());
        assert_eq!(
            relative,
            abs("inputs/x.fit"),
            "taken against the working directory"
        );
        assert_eq!(dialogs.dir(), None, "no answer left");
    }
}
