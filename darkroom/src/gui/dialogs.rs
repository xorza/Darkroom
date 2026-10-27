//! [`Dialogs`]: the file pickers `App` asks for paths through — the answers a driver queued, then
//! native ones (rfd) in the window. Project byte⇄type plumbing lives in `crate::core::io::document`;
//! this side hands paths off to that GUI-free module. Failures degrade — a cancelled or failed
//! pick returns `None` — rather than crashing. Everything else the shell does for us is in
//! `crate::platform`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::fs;

use crate::core::io::document::document_format::DocumentFormat;

/// Where `App`'s path picks are answered: the answers a driver queued, one per ask in order, then
/// the fallback. A queued answer of the other shape reads as a cancel and logs an error, which the
/// driver reports. `darkroom/AGENTS.md`, "The editor harness", "A dialog takes a queued
/// answer, else asks the person".
#[derive(Debug)]
pub(crate) struct Dialogs {
    queued: VecDeque<DialogAnswer>,
    fallback: DialogFallback,
    /// Raised while a native dialog is open, which blocks the main thread until the person
    /// answers: a driver on another thread reads it to say why the editor does not answer.
    native_open: Arc<AtomicBool>,
}

/// One scripted answer to a file dialog, which the next dialog the app opens takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogAnswer {
    /// One path, for every ask but the multi-file pick.
    Path(PathBuf),
    /// The multi-file pick's paths.
    Paths(Vec<PathBuf>),
    /// The user closed the dialog.
    Cancel,
}

/// What answers an ask with no queued answer left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogFallback {
    /// The platform's file dialogs, which block until the person answers.
    Native,
    /// A cancel, and an error for the driver to report: a session no one can see must not block
    /// on a dialog.
    Refuse,
}

/// One file type a save dialog offers: the label of its filter, and its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileType<'a> {
    label: &'a str,
    extension: &'a str,
}

/// A kind of file a save names: the file types its dialog offers, of which the first is the one
/// a typed name without any of their extensions takes, as the native dialogs add none. A kind
/// of several forms offers each as a type, as Inkscape offers SVG and compressed SVG: rfd reports
/// the path alone, not the filter picked, so the extension is what carries the choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SavedFile<'a> {
    types: &'a [FileType<'a>],
}

const PLAIN_PROJECT: FileType<'static> = FileType {
    label: "Darkroom project",
    extension: DocumentFormat::Ron.extension(),
};

const COMPRESSED_PROJECT: FileType<'static> = FileType {
    label: "Compressed Darkroom project",
    extension: DocumentFormat::ZippedRon.extension(),
};

impl<'a> SavedFile<'a> {
    /// A project, `format`'s type first: the dialog opens on it and a bare name takes it, so a
    /// Save As of a compressed project stays compressed unless the person picks otherwise.
    pub(crate) const fn project(format: DocumentFormat) -> Self {
        match format {
            DocumentFormat::Ron => Self {
                types: &[PLAIN_PROJECT, COMPRESSED_PROJECT],
            },
            DocumentFormat::ZippedRon => Self {
                types: &[COMPRESSED_PROJECT, PLAIN_PROJECT],
            },
        }
    }

    pub(crate) const PNG: Self = Self {
        types: &[FileType {
            label: "PNG image",
            extension: "png",
        }],
    };

    /// The type a name without any of this kind's extensions takes.
    const fn default_type(self) -> FileType<'a> {
        self.types[0]
    }

    /// `path` with one of this kind's extensions: the first type's in place of any other, and one
    /// that matches without regard to case kept as typed.
    fn extended(self, mut path: PathBuf) -> PathBuf {
        let has = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                self.types
                    .iter()
                    .any(|kind| extension.eq_ignore_ascii_case(kind.extension))
            });
        if !has {
            path.set_extension(self.default_type().extension);
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

impl Dialogs {
    /// No answers queued, `fallback` behind them.
    pub(crate) fn new(fallback: DialogFallback) -> Self {
        Self {
            queued: VecDeque::new(),
            fallback,
            native_open: Arc::default(),
        }
    }

    /// The flag raised while a native dialog is open.
    pub(crate) fn native_open(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.native_open)
    }

    /// Queue `answer` for the next ask the queue reaches.
    pub(crate) fn queue(&mut self, answer: DialogAnswer) {
        self.queued.push_back(answer);
    }

    /// A project file to open, the picker starting beside `start`.
    pub(crate) fn project_open_path(&mut self, start: Option<&Path>) -> Option<PathBuf> {
        self.path("open a project", || pick_project_open_path(start))
    }

    /// Where to save a file of `kind`, the picker starting beside `start`; the extension is added.
    pub(crate) fn save_path(
        &mut self,
        kind: SavedFile<'_>,
        start: Option<&Path>,
    ) -> Option<PathBuf> {
        self.save_as(
            &format!("save a {}", kind.default_type().label),
            kind,
            start,
        )
    }

    /// One existing file with one of `extensions`, or any file for none.
    pub(crate) fn existing_file(&mut self, extensions: &[&str]) -> Option<PathBuf> {
        self.path("pick a file", || pick_existing_file(extensions))
    }

    /// Existing files with one of `extensions`, sorted, each once, at least one.
    pub(crate) fn existing_files(&mut self, extensions: &[&str]) -> Option<Vec<PathBuf>> {
        const ASK: &str = "pick files";
        match self.queued.pop_front() {
            Some(DialogAnswer::Paths(paths)) => {
                normalize_file_selection(paths.into_iter().map(absolute).collect())
            }
            Some(DialogAnswer::Cancel) => None,
            Some(answer) => unanswered(ASK, Some(&answer)),
            None => self.fall_back(ASK, || pick_existing_files(extensions)),
        }
    }

    /// A file to create, with one of `extensions`. With exactly one, a name without it gets it
    /// and an existing extended name is confirmed, as a save's is; with several no one of them is
    /// the name's, so the name stays as typed.
    pub(crate) fn new_file(&mut self, extensions: &[&str]) -> Option<PathBuf> {
        const ASK: &str = "name a new file";
        match extensions {
            [extension] => self.save_as(
                ASK,
                SavedFile {
                    types: &[FileType {
                        label: "Files",
                        extension,
                    }],
                },
                None,
            ),
            _ => self.path(ASK, || pick_new_file(extensions)),
        }
    }

    /// A directory.
    pub(crate) fn dir(&mut self) -> Option<PathBuf> {
        self.path("pick a folder", pick_dir)
    }

    /// A save of `kind` that `ask` names: the next queued answer, else the native save dialog
    /// behind the fallback, either with the extension added.
    fn save_as(&mut self, ask: &str, kind: SavedFile<'_>, start: Option<&Path>) -> Option<PathBuf> {
        match self.queued.pop_front() {
            Some(answer) => queued_path(ask, answer).map(|path| kind.extended(path)),
            None => self.fall_back(ask, || pick_save_path(kind, start)),
        }
    }

    /// A one-path ask: the next queued answer, else `native` behind the fallback.
    fn path(&mut self, ask: &str, native: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
        match self.queued.pop_front() {
            Some(answer) => queued_path(ask, answer),
            None => self.fall_back(ask, native),
        }
    }

    /// `native` for [`DialogFallback::Native`], the flag raised while it runs, else a refusal
    /// of `ask`.
    fn fall_back<T>(&self, ask: &str, native: impl FnOnce() -> Option<T>) -> Option<T> {
        match self.fallback {
            DialogFallback::Native => {
                self.native_open.store(true, Ordering::Release);
                let answer = native();
                self.native_open.store(false, Ordering::Release);
                answer
            }
            DialogFallback::Refuse => unanswered(ask, None),
        }
    }
}

/// A queued answer to a one-path ask.
fn queued_path(ask: &str, answer: DialogAnswer) -> Option<PathBuf> {
    match answer {
        DialogAnswer::Path(path) => Some(absolute(path)),
        DialogAnswer::Cancel => None,
        DialogAnswer::Paths(_) => unanswered(ask, Some(&answer)),
    }
}

/// `path` as a native dialog answers it: absolute, a relative one taken against the working
/// directory, so a queued pick holds what a document keeps in memory.
fn absolute(path: PathBuf) -> PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

/// An ask with no answer of its shape: a cancel, and an error for the driver to report.
fn unanswered<T>(ask: &str, answer: Option<&DialogAnswer>) -> Option<T> {
    match answer {
        None => tracing::error!("a dialog asked to {ask}, and no queued answer was left"),
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
        .add_filter(
            "Darkroom project",
            &DocumentFormat::ALL.map(DocumentFormat::extension),
        )
        .pick_file()
}

/// The native dialog confirms overwriting the name the user typed, not the one
/// [`SavedFile::extended`] makes of it — so when that one already exists,
/// the dialog asks again with it filled in, and its own overwrite prompt
/// answers for the file that will be replaced. A second save dialog rather than
/// a message box, because rfd shows message boxes on Linux through zenity,
/// which a desktop may lack, and the save then fails with nothing on screen.
fn pick_save_path(kind: SavedFile<'_>, start: Option<&Path>) -> Option<PathBuf> {
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

fn save_dialog(kind: SavedFile<'_>, start: Option<&Path>) -> rfd::FileDialog {
    kind.types.iter().fold(file_dialog(start), |dialog, kind| {
        dialog.add_filter(kind.label, &[kind.extension])
    })
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

    use crate::core::io::document::document_format::DocumentFormat;
    use crate::gui::dialogs::{
        DialogAnswer, DialogFallback, Dialogs, SavedFile, normalize_file_selection,
    };

    /// A save adds its kind's extension to a bare name and puts it in place of another, and keeps
    /// one that matches without regard to case. Only an extended name that exists and differs
    /// from the pick is owed a second confirmation: a bare name over nothing, and a pick that
    /// already carries the extension (the dialog asked about that very file), are not.
    #[test]
    fn only_an_existing_extended_name_needs_confirming_again() {
        let extended = |kind: SavedFile<'_>, name: &str| kind.extended(PathBuf::from(name));
        assert_eq!(
            extended(SavedFile::project(DocumentFormat::Ron), "scene"),
            PathBuf::from("scene.darkroom")
        );
        assert_eq!(
            extended(SavedFile::project(DocumentFormat::Ron), "scene.json"),
            PathBuf::from("scene.darkroom")
        );
        assert_eq!(
            extended(SavedFile::project(DocumentFormat::Ron), "scene.DARKROOM"),
            PathBuf::from("scene.DARKROOM")
        );
        assert_eq!(
            extended(SavedFile::project(DocumentFormat::Ron), "scene.darkroomz"),
            PathBuf::from("scene.darkroomz"),
            "the compressed file type is the project's too"
        );
        assert_eq!(
            extended(SavedFile::project(DocumentFormat::Ron), "scene.DARKROOMZ"),
            PathBuf::from("scene.DARKROOMZ")
        );
        assert_eq!(
            extended(SavedFile::project(DocumentFormat::ZippedRon), "scene"),
            PathBuf::from("scene.darkroomz"),
            "a bare name takes the open project's format"
        );
        assert_eq!(
            extended(
                SavedFile::project(DocumentFormat::ZippedRon),
                "scene.darkroom"
            ),
            PathBuf::from("scene.darkroom"),
            "and the other type stays the person's pick"
        );
        assert_eq!(extended(SavedFile::PNG, "view"), PathBuf::from("view.png"));
        assert_eq!(
            extended(SavedFile::PNG, "view.darkroom"),
            PathBuf::from("view.png"),
            "another kind's extension is replaced"
        );
        assert_eq!(
            extended(SavedFile::PNG, "view.PNG"),
            PathBuf::from("view.PNG")
        );

        let project = SavedFile::project(DocumentFormat::Ron);
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

    /// Queued answers go to the asks in order, each of its own shape and absolute, as a native
    /// dialog's are: a relative one is taken against the working directory. The project save adds
    /// the extension, as the native one does, and so does a new file of one extension; the file
    /// list comes back sorted and once each. A
    /// cancel, an answer of the other shape, and an ask past the last answer all read as `None`,
    /// the last one from the refusing fallback; an answer queued after that goes to the next ask.
    #[test]
    fn queued_answers_go_to_the_asks_in_order_before_the_fallback() {
        // As a native dialog answers: absolute, so `/p` on Windows takes the working drive.
        let abs = |path: &str| path::absolute(path).unwrap();
        let mut dialogs = Dialogs::new(DialogFallback::Refuse);
        for answer in [
            DialogAnswer::Path(PathBuf::from("/p/scene.darkroom")),
            DialogAnswer::Path(PathBuf::from("/p/copy")),
            DialogAnswer::Paths(vec![PathBuf::from("/d/b.fit"), PathBuf::from("/d/a.fit")]),
            DialogAnswer::Cancel,
            DialogAnswer::Paths(vec![PathBuf::from("/d/c.fit")]),
            DialogAnswer::Path(PathBuf::from("/d")),
            DialogAnswer::Path(PathBuf::from("inputs/x.fit")),
            DialogAnswer::Path(PathBuf::from("/o/out")),
            DialogAnswer::Path(PathBuf::from("/o/out")),
        ] {
            dialogs.queue(answer);
        }
        assert_eq!(
            dialogs.project_open_path(None),
            Some(abs("/p/scene.darkroom"))
        );
        assert_eq!(
            dialogs.save_path(SavedFile::project(DocumentFormat::Ron), None),
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
        assert_eq!(
            dialogs.new_file(&["png"]),
            Some(abs("/o/out.png")),
            "one extension is added"
        );
        assert_eq!(
            dialogs.new_file(&["png", "tiff"]),
            Some(abs("/o/out")),
            "of several none is the name's"
        );
        assert_eq!(dialogs.dir(), None, "no answer left");
        assert_eq!(dialogs.existing_files(&[]), None, "no answer left");

        dialogs.queue(DialogAnswer::Path(PathBuf::from("/q")));
        assert_eq!(dialogs.dir(), Some(abs("/q")), "a later answer wins again");
    }
}
