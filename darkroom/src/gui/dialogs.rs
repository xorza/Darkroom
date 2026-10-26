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

/// Where `App`'s path picks are answered.
#[derive(Debug)]
pub(crate) enum Dialogs {
    /// The platform's file dialogs, which block until the user answers.
    Native,
    /// Answers given ahead, one per ask in order: a session no one can see must not block on a
    /// dialog. An ask with no answer left, or with an answer of the other shape, reads as a
    /// cancel and logs an error, which the harness reports.
    #[cfg(feature = "internals")]
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "made by the harness, which lands after this seam")
    )]
    Scripted(VecDeque<DialogAnswer>),
}

/// One scripted answer to a dialog.
#[cfg(feature = "internals")]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "made by the harness, which lands after this seam")
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DialogAnswer {
    /// One path, for every ask but the multi-file pick.
    Path(PathBuf),
    /// The multi-file pick's paths.
    Paths(Vec<PathBuf>),
    /// The user closed the dialog.
    Cancel,
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

    /// Where to save the project, the picker starting beside `start`; the extension is added.
    pub(crate) fn project_save_path(&mut self, start: Option<&Path>) -> Option<PathBuf> {
        match self {
            Self::Native => pick_project_save_path(start),
            #[cfg(feature = "internals")]
            Self::Scripted(answers) => {
                scripted_path(answers, "save the project").map(document::with_extension)
            }
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
                Some(DialogAnswer::Paths(paths)) => normalize_file_selection(paths),
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
        Some(DialogAnswer::Path(path)) => Some(path),
        Some(DialogAnswer::Cancel) => None,
        answer => unanswered(ask, answer.as_ref()),
    }
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
/// [`document::with_extension`] makes of it — so when that one already exists,
/// the dialog asks again with it filled in, and its own overwrite prompt
/// answers for the file that will be replaced. A second save dialog rather than
/// a message box, because rfd shows message boxes on Linux through zenity,
/// which a desktop may lack, and the save then fails with nothing on screen.
fn pick_project_save_path(start: Option<&Path>) -> Option<PathBuf> {
    let mut picked = project_save_dialog(start).save_file()?;
    while let Some(target) = unconfirmed_target(&picked) {
        let mut dialog = project_save_dialog(Some(&target));
        if let Some(name) = target.file_name().and_then(|name| name.to_str()) {
            dialog = dialog.set_file_name(name);
        }
        picked = dialog.save_file()?;
    }
    Some(document::with_extension(picked))
}

fn project_save_dialog(start: Option<&Path>) -> rfd::FileDialog {
    file_dialog(start).add_filter("Darkroom project", &[document::EXTENSION])
}

/// The file a save to `picked` would replace without the dialog having asked
/// about it: the extended name, when it differs from the picked one and exists.
fn unconfirmed_target(picked: &Path) -> Option<PathBuf> {
    let target = document::with_extension(picked.to_path_buf());
    (target != picked && fs::exists(&target)).then_some(target)
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
    use std::path::PathBuf;

    use common::{TempDir, fs};

    use std::collections::VecDeque;

    use crate::gui::dialogs::{
        DialogAnswer, Dialogs, normalize_file_selection, unconfirmed_target,
    };

    /// Only an extended name that exists and differs from the pick is owed a
    /// second confirmation: a bare name over nothing, and a pick that already
    /// carries the extension (the dialog asked about that very file), are not.
    #[test]
    fn only_an_existing_extended_name_needs_confirming_again() {
        let dir = TempDir::new("darkroom-save-target");
        let bare = dir.join("scene");
        let extended = dir.join("scene.darkroom");
        assert_eq!(unconfirmed_target(&bare), None, "nothing to replace yet");

        fs::write(&extended, b"x").unwrap();
        assert_eq!(unconfirmed_target(&bare), Some(extended.clone()));
        assert_eq!(
            unconfirmed_target(&extended),
            None,
            "the dialog confirmed this file itself"
        );
        assert_eq!(
            unconfirmed_target(&dir.join("scene.DARKROOM")),
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

    /// Scripted answers go to the asks in order, each of its own shape. The project save adds the
    /// extension, as the native one does; the file list comes back sorted and once each. A cancel,
    /// an answer of the other shape, and an ask past the last answer all read as `None`.
    #[test]
    fn scripted_answers_go_to_the_asks_in_order() {
        let mut dialogs = Dialogs::Scripted(VecDeque::from([
            DialogAnswer::Path(PathBuf::from("/p/scene.darkroom")),
            DialogAnswer::Path(PathBuf::from("/p/copy")),
            DialogAnswer::Paths(vec![PathBuf::from("/d/b.fit"), PathBuf::from("/d/a.fit")]),
            DialogAnswer::Cancel,
            DialogAnswer::Paths(vec![PathBuf::from("/d/c.fit")]),
            DialogAnswer::Path(PathBuf::from("/d")),
        ]));
        assert_eq!(
            dialogs.project_open_path(None),
            Some(PathBuf::from("/p/scene.darkroom"))
        );
        assert_eq!(
            dialogs.project_save_path(None),
            Some(PathBuf::from("/p/copy.darkroom"))
        );
        assert_eq!(
            dialogs.existing_files(&["fit"]),
            Some(vec![PathBuf::from("/d/a.fit"), PathBuf::from("/d/b.fit")])
        );
        assert_eq!(dialogs.new_file(&[]), None, "cancelled");
        assert_eq!(
            dialogs.existing_file(&["fit"]),
            None,
            "a list for a one-file ask"
        );
        assert_eq!(dialogs.dir(), Some(PathBuf::from("/d")));
        assert_eq!(dialogs.dir(), None, "no answer left");
    }
}
