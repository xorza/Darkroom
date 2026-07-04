//! The shared subgraph library: `Linked` subgraph defs that live in the
//! runtime `Library` (where `SubgraphRef::Linked` resolves) rather than in
//! any one document, so they're reusable across documents. Persisted in
//! the working dir as Rhai, like the preferences — loaded into
//! `library.subgraphs` at startup, saved when "promote" grows it.

use std::path::PathBuf;

use common::{SerdeFormat, deserialize, serialize};
use scenarium::graph::subgraph::SubgraphDef;

/// Library file name, resolved relative to the process working directory.
/// Rhai so it's hand-editable and matches the doc / theme / preferences format.
const LIBRARY_FILE: &str = "darkroom.library.rhai";

/// On-disk wrapper so the file is a named table rather than a bare
/// collection (more robust to hand-edit and future fields). A plain
/// `Vec` — each def carries its own `id`, and we only ever iterate to
/// feed `Library::add_subgraph`.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct LibraryFile {
    #[serde(default)]
    subgraphs: Vec<SubgraphDef>,
}

fn path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join(LIBRARY_FILE)
}

/// Load the shared subgraph defs from the working dir. Any failure
/// (missing file, parse error) degrades to an empty library rather than
/// blocking startup.
pub(crate) fn load_library() -> Vec<SubgraphDef> {
    let Ok(bytes) = std::fs::read(path()) else {
        return Vec::new();
    };
    deserialize::<LibraryFile>(&bytes, SerdeFormat::Rhai)
        .map(|lib| lib.subgraphs)
        .unwrap_or_default()
}

/// Write the shared subgraph defs to the working dir. Errors print to
/// stderr — a failed persist shouldn't interrupt the session.
pub(crate) fn save_library<'a>(subgraphs: impl Iterator<Item = &'a SubgraphDef>) {
    let lib = LibraryFile {
        subgraphs: subgraphs.cloned().collect(),
    };
    let bytes = match serialize(&lib, SerdeFormat::Rhai) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("library save failed: {err}");
            return;
        }
    };
    if let Err(err) = std::fs::write(path(), &bytes) {
        eprintln!("library save failed: {err}");
    }
}
