use common::TempDir;

use super::*;
use crate::core::document::internals::DocFixture;
use std::fs;

#[test]
fn document_round_trips_as_one_ron_entry() {
    let dir = TempDir::new("darkroom-document-roundtrip");
    let path = dir.join("roundtrip.darkroom");
    // Populated, not `Document::default()`: an empty document has an empty
    // placement map, which would never exercise a `NodeId` as a key.
    let document = DocFixture::sample().doc;
    assert!(
        !document.main_view.item_placements.is_empty(),
        "the fixture must place nodes for this to prove anything"
    );

    save(&document, &path).expect("save document");
    assert_eq!(load(&path).expect("load document"), document);

    let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
    assert_eq!(archive.len(), 1, "archives contain only the document entry");
    let mut entry = archive.by_name(DOCUMENT_ENTRY).unwrap();
    assert_eq!(entry.compression(), CompressionMethod::Deflated);
    let mut encoded = String::new();
    entry.read_to_string(&mut encoded).unwrap();
    let decoded: Document = ron::from_str(&encoded).unwrap();
    assert_eq!(decoded, document, "the archive payload is plain RON");
}

#[test]
fn document_extension_is_required_case_insensitively() {
    let document = Document::default();
    let dir = TempDir::new("darkroom-document-extension");
    let wrong = dir.join("wrong.json");
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

    let uppercase = dir.join("uppercase.DARKROOM");
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
fn load_rejects_invalid_archives_and_malformed_documents() {
    let dir = TempDir::new("darkroom-document-rejects");
    let corrupt = dir.join("corrupt.darkroom");
    fs::write(&corrupt, b"not a zip archive").unwrap();
    assert!(
        matches!(
            load(&corrupt).unwrap_err(),
            DocumentLoadError::InvalidArchive { path, .. } if path == corrupt
        ),
        "corrupt ZIP reports the exact archive path"
    );

    let missing = dir.join("missing.darkroom");
    write_test_archive(&missing, "other.ron", b"()");
    assert!(
        matches!(
            load(&missing).unwrap_err(),
            DocumentLoadError::DocumentEntryCount { path, count: 0 } if path == missing
        ),
        "missing document entry reports its archive and exact count"
    );

    let malformed = dir.join("malformed.darkroom");
    write_test_archive(&malformed, DOCUMENT_ENTRY, b"{");
    assert!(
        matches!(
            load(&malformed).unwrap_err(),
            DocumentLoadError::DeserializeDocument { path, .. } if path == malformed
        ),
        "malformed RON reports its archive"
    );
}

/// The one size predicate a load and a save share admits the limit itself and
/// refuses the first byte over it.
#[test]
fn document_size_limit_rejects_the_first_byte_over_the_boundary() {
    assert!(fits(MAX_DOCUMENT_BYTES));
    assert!(!fits(MAX_DOCUMENT_BYTES + 1));
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
