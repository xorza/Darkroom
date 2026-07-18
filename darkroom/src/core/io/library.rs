//! The shared subgraph library: `Linked` subgraph defs that live in the
//! runtime `Library` (where `SubgraphRef::Linked` resolves) rather than in
//! any one document, so they're reusable across documents. Persisted in
//! the working dir as Rhai, like the preferences — loaded into
//! `library.subgraphs` at startup, saved when "promote" grows it. Failures
//! surface through the caller (`Engine`) into the status log.
//!
//! A file that fails to load still holds defs a save would destroy (the
//! session degraded to an empty library in its place), so [`load_library`]
//! moves it aside to `<name>.broken` for recovery, and [`save_library`]
//! refuses to overwrite anything it can't re-read — the backstop for a
//! failed move or corruption after startup.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use common::{SerdeFormat, deserialize, serialize};
use scenarium::SubgraphDef;

use crate::core::io::cwd_file;

/// Library file name, resolved relative to the process working directory.
/// Rhai so it's hand-editable and matches the doc / theme / preferences format.
const LIBRARY_FILE: &str = "darkroom.library.rhai";

/// On-disk wrapper so the file is a named table rather than a bare
/// collection (more robust to hand-edit and future fields). A plain
/// `Vec` — each def carries its own `id`, and we only ever iterate to
/// feed `Library::insert_subgraph`.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct LibraryFile {
    #[serde(default)]
    subgraphs: Vec<SubgraphDef>,
}

fn path() -> PathBuf {
    cwd_file(LIBRARY_FILE)
}

/// Where a failed-to-load library file is moved so a later save can't
/// destroy it: `darkroom.library.rhai` → `darkroom.library.rhai.broken`.
fn broken_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .expect("library path has a file name")
        .to_os_string();
    name.push(".broken");
    path.with_file_name(name)
}

/// Read + parse the library file, structurally checking every def — the
/// file is hand-editable, so it's untrusted input gated like a document.
/// A missing file is the normal first-launch case (`Ok(empty)`); any
/// other failure is `Err` (render with `{:#}` for the full reason). No
/// side effects — the recovery move is [`load_library`]'s.
fn read_defs(path: &Path) -> Result<Vec<SubgraphDef>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| path.display().to_string()),
    };
    let lib = deserialize::<LibraryFile>(&bytes, SerdeFormat::Rhai)
        .with_context(|| path.display().to_string())?;
    for def in &lib.subgraphs {
        def.check()
            .with_context(|| format!("{}: invalid subgraph {:?}", path.display(), def.name))?;
    }
    Ok(lib.subgraphs)
}

/// Load the shared subgraph defs from the working dir. On a failed load the
/// session degrades to an empty library, so the file is moved aside to
/// `<name>.broken` — recoverable by the user, and no longer in the way of
/// the saves that would otherwise overwrite it with that empty set. The
/// `Err` carries the reason plus where the file went.
pub(crate) fn load_library() -> Result<Vec<SubgraphDef>> {
    load_library_from(&path())
}

fn load_library_from(path: &Path) -> Result<Vec<SubgraphDef>> {
    read_defs(path).map_err(|err| {
        let broken = broken_path(path);
        // Flatten the chain (`:#`) so the recovery note reads after the
        // reason, not as an outer context in front of it.
        match std::fs::rename(path, &broken) {
            Ok(()) => anyhow!("{err:#} — file moved to {}", broken.display()),
            Err(rename_err) => {
                anyhow!("{err:#} — couldn't move the file aside ({rename_err}); fix or remove it")
            }
        }
    })
}

/// Write the shared subgraph defs to the working dir, refusing to overwrite
/// a file whose defs can't be re-read (they'd be destroyed). The caller
/// (`Engine::edit_library`) reports the error to the status log, which also
/// traces it.
pub(crate) fn save_library<'a>(subgraphs: impl Iterator<Item = &'a SubgraphDef>) -> Result<()> {
    save_library_to(&path(), subgraphs)
}

fn save_library_to<'a>(
    path: &Path,
    subgraphs: impl Iterator<Item = &'a SubgraphDef>,
) -> Result<()> {
    // Normally a bad file was already moved aside at load; this closes the
    // gaps (a failed move, corruption after startup).
    if let Err(err) = read_defs(path) {
        bail!("refusing to overwrite a library file that can't be re-read — {err:#}");
    }
    let lib = LibraryFile {
        subgraphs: subgraphs.cloned().collect(),
    };
    let bytes = serialize(&lib, SerdeFormat::Rhai)?;
    std::fs::write(path, &bytes).with_context(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use common::test_utils::test_output_path;
    use scenarium::NodeId;
    use scenarium::{SubgraphEvent, SubgraphId};

    use super::*;

    fn def(name: &str) -> SubgraphDef {
        SubgraphDef::new(SubgraphId::unique(), name).category("test")
    }

    #[test]
    fn save_load_roundtrip() {
        let path = test_output_path("darkroom_library/roundtrip.rhai");
        // A stale file from an earlier run would trip save's re-read guard.
        let _ = std::fs::remove_file(&path);
        let defs = [def("blur"), def("sharpen")];
        save_library_to(&path, defs.iter()).unwrap();

        assert_eq!(load_library_from(&path).unwrap(), defs);
    }

    #[test]
    fn missing_file_is_empty_and_not_an_error() {
        let path = test_output_path("darkroom_library/never-written.rhai");
        assert_eq!(load_library_from(&path).unwrap(), []);
    }

    #[test]
    fn corrupt_file_is_quarantined_and_the_slot_reusable() {
        let path = test_output_path("darkroom_library/corrupt.rhai");
        let broken = broken_path(&path);
        let garbage = "#{ subgraphs: [ truncated";
        std::fs::write(&path, garbage).unwrap();

        let err = format!("{:#}", load_library_from(&path).unwrap_err());
        assert!(
            err.contains(path.to_str().unwrap()) && err.contains(broken.to_str().unwrap()),
            "error names the file and the backup: {err}"
        );
        assert!(!path.exists(), "the corrupt file was moved aside");
        assert_eq!(
            std::fs::read_to_string(&broken).unwrap(),
            garbage,
            "the backup preserves the corrupt content byte-for-byte"
        );

        // With the bad file out of the way, the save→load cycle works again
        // and the backup is untouched.
        let defs = [def("recovered")];
        save_library_to(&path, defs.iter()).unwrap();
        assert_eq!(load_library_from(&path).unwrap(), defs);
        assert_eq!(std::fs::read_to_string(&broken).unwrap(), garbage);
    }

    #[test]
    fn structurally_invalid_def_is_quarantined_like_a_parse_failure() {
        // Parses fine but fails `SubgraphDef::check` (dangling event
        // emitter): the load refuses it and moves the file aside, exactly
        // like syntactic corruption.
        let path = test_output_path("darkroom_library/invalid-def.rhai");
        // A stale file from an earlier run would trip save's re-read guard.
        let _ = std::fs::remove_file(&path);
        let mut bad = def("dangling");
        bad.events.push(SubgraphEvent {
            name: "tick".into(),
            emitter: NodeId::unique(),
            emitter_event_idx: 0,
        });
        save_library_to(&path, [bad].iter()).unwrap();

        let err = format!("{:#}", load_library_from(&path).unwrap_err());
        assert!(
            err.contains("invalid subgraph") && err.contains("dangling"),
            "error names the bad def: {err}"
        );
        assert!(!path.exists(), "the invalid file was moved aside");
    }

    #[test]
    fn save_refuses_to_overwrite_an_unreadable_file() {
        let path = test_output_path("darkroom_library/corrupt-at-save.rhai");
        let garbage = "not a library";
        std::fs::write(&path, garbage).unwrap();

        let err = format!(
            "{:#}",
            save_library_to(&path, [def("x")].iter()).unwrap_err()
        );
        assert!(
            err.contains(path.to_str().unwrap()),
            "error names the file: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            garbage,
            "the refused save left the file untouched"
        );
    }

    #[test]
    fn unwritable_path_reports_save_failure() {
        let path = test_output_path("darkroom_library/no-such-dir").join("lib.rhai");
        let err = format!(
            "{:#}",
            save_library_to(&path, [def("x")].iter()).unwrap_err()
        );
        assert!(
            err.contains(path.to_str().unwrap()),
            "error names the file: {err}"
        );
    }
}
