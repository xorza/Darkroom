use std::io::Read as _;

use common::internals::test_output_path;
use scenarium::{Binding, InputPort, NodeId, StaticValue};

use super::*;

#[test]
fn document_round_trips_as_one_json_entry() {
    let path = test_output_path("darkroom_document/roundtrip.darkroom");
    let document = Document::default();

    save(&document, &path).expect("save document");
    assert_eq!(load(&path).expect("load document"), document);

    let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
    assert_eq!(archive.len(), 1, "archives contain only the document entry");
    let mut entry = archive.by_name(DOCUMENT_ENTRY).unwrap();
    assert_eq!(entry.compression(), CompressionMethod::Deflated);
    let mut json = String::new();
    entry.read_to_string(&mut json).unwrap();
    let decoded: Document = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, document, "the archive payload is plain JSON");
}

#[test]
fn document_extension_is_required_case_insensitively() {
    let document = Document::default();
    let wrong = test_output_path("darkroom_document/wrong.json");
    assert!(matches!(
        save(&document, &wrong).unwrap_err(),
        DocumentSaveError::InvalidExtension { path } if path == wrong
    ));
    assert!(
        matches!(
            load(&wrong).unwrap_err(),
            DocumentLoadError::InvalidExtension { path } if path == wrong
        ),
        "load reports the exact rejected path"
    );

    let uppercase = test_output_path("darkroom_document/uppercase.DARKROOM");
    save(&document, &uppercase).expect("uppercase extension is valid");
    assert_eq!(load(&uppercase).unwrap(), document);

    assert_eq!(
        with_extension(PathBuf::from("scene")),
        PathBuf::from("scene.darkroom")
    );
    assert_eq!(
        with_extension(PathBuf::from("scene.json")),
        PathBuf::from("scene.darkroom")
    );
    assert_eq!(
        with_extension(PathBuf::from("scene.DARKROOM")),
        PathBuf::from("scene.DARKROOM")
    );
}

#[test]
fn save_refuses_an_invalid_document_and_leaves_the_file_alone() {
    // Save validates with the same predicate as load, so a document
    // the next launch would refuse can never replace the one on disk.
    // Before this, save only asserted in debug builds — a release
    // build wrote the bad project happily and failed at reopen.
    let path = test_output_path("darkroom_document/refused.darkroom");
    let good = Document::default();
    save(&good, &path).expect("a valid document saves");
    let on_disk = std::fs::read(&path).unwrap();

    // A binding whose consumer node isn't in the graph — structurally
    // invalid without tripping an insertion assert.
    let mut bad = Document::default();
    bad.graph.bindings.insert(
        InputPort::new(NodeId::unique(), 0),
        Binding::Const(StaticValue::Int(1)),
    );
    assert!(
        matches!(
            save(&bad, &path).unwrap_err(),
            DocumentSaveError::InvalidDocument { path: p, source }
                if p == path && source.to_string().contains("binding")
        ),
        "the refusal names the path and the reason"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        on_disk,
        "the previously saved document is still intact"
    );
    assert_eq!(load(&path).unwrap(), good, "and still loads");
}

#[test]
fn load_rejects_invalid_archives_and_missing_or_invalid_documents() {
    let corrupt = test_output_path("darkroom_document/corrupt.darkroom");
    std::fs::write(&corrupt, b"not a zip archive").unwrap();
    assert!(
        matches!(
            load(&corrupt).unwrap_err(),
            DocumentLoadError::InvalidArchive { path, .. } if path == corrupt
        ),
        "corrupt ZIP reports the exact archive path"
    );

    let missing = test_output_path("darkroom_document/missing.darkroom");
    write_test_archive(&missing, "other.json", b"{}");
    assert!(
        matches!(
            load(&missing).unwrap_err(),
            DocumentLoadError::DocumentEntryCount { path, count: 0 } if path == missing
        ),
        "missing document entry reports its archive and exact count"
    );

    let malformed = test_output_path("darkroom_document/malformed.darkroom");
    write_test_archive(&malformed, DOCUMENT_ENTRY, b"{");
    assert!(
        matches!(
            load(&malformed).unwrap_err(),
            DocumentLoadError::DeserializeDocument { path, .. } if path == malformed
        ),
        "malformed JSON reports its archive"
    );

    let invalid = test_output_path("darkroom_document/invalid.darkroom");
    let mut document = Document::default();
    document.graph.bindings.insert(
        InputPort::new(NodeId::unique(), 0),
        Binding::Const(StaticValue::Int(1)),
    );
    let json = serde_json::to_vec(&document).unwrap();
    write_test_archive(&invalid, DOCUMENT_ENTRY, &json);
    assert!(
        matches!(
            load(&invalid).unwrap_err(),
            DocumentLoadError::InvalidDocument { path, source }
                if path == invalid && source.to_string().contains("binding")
        ),
        "structural validation retains the archive path and reason"
    );
}

#[test]
fn document_size_limit_rejects_the_first_byte_over_the_boundary() {
    let path = Path::new("oversized.darkroom");
    ensure_save_document_size(path, MAX_DOCUMENT_BYTES).expect("save boundary is accepted");
    assert!(matches!(
        ensure_save_document_size(path, MAX_DOCUMENT_BYTES + 1).unwrap_err(),
        DocumentSaveError::DocumentTooLarge {
            path: error_path,
            size
        } if error_path == path && size == MAX_DOCUMENT_BYTES + 1
    ));

    ensure_load_document_size(path, MAX_DOCUMENT_BYTES).expect("load boundary is accepted");
    assert!(matches!(
        ensure_load_document_size(path, MAX_DOCUMENT_BYTES + 1).unwrap_err(),
        DocumentLoadError::DocumentTooLarge {
            path: error_path,
            size
        } if error_path == path && size == MAX_DOCUMENT_BYTES + 1
    ));
}

fn write_test_archive(path: &Path, name: &str, contents: &[u8]) {
    let file = File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    archive
        .start_file(name, SimpleFileOptions::default())
        .unwrap();
    archive.write_all(contents).unwrap();
    archive.finish().unwrap();
}
