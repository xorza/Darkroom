use std::panic::{self, AssertUnwindSafe};
use std::path::{self, PathBuf};

use common::{TempDir, fs};

use crate::core::io::document::document_format::DocumentFormat;
use crate::gui::dialogs::{
    DialogAnswer, DialogFallback, Dialogs, NativeDialogFlag, SavedFile, normalize_file_selection,
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

/// The native fallback raises its flag while the dialog runs and lowers it after, also when the
/// dialog panics; the refusing fallback runs no dialog.
#[test]
fn the_native_flag_is_raised_only_while_a_dialog_runs() {
    let flag = NativeDialogFlag::default();
    let dialogs = Dialogs::new(DialogFallback::Native(flag.clone()));
    assert!(!flag.is_raised());
    assert_eq!(
        dialogs.fall_back("ask", || Some(flag.is_raised())),
        Some(true)
    );
    assert!(!flag.is_raised());

    let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
        dialogs.fall_back("ask", || -> Option<()> { panic!("the dialog failed") })
    }));
    assert!(unwound.is_err());
    assert!(!flag.is_raised(), "a panic lowers the flag");

    let refused = Dialogs::new(DialogFallback::Refuse);
    assert_eq!(
        refused.fall_back("ask", || -> Option<()> {
            unreachable!("a refusal runs no dialog")
        }),
        None
    );
}
