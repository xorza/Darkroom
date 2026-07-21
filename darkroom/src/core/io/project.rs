//! Darkroom project archives: one validated JSON document inside a ZIP file.

use std::fs::File;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use common::file_utils;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::core::document::Document;

pub(crate) const EXTENSION: &str = "darkroom";
const DOCUMENT_ENTRY: &str = "project.json";
const MAX_DOCUMENT_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) fn load(path: &Path) -> Result<Document> {
    ensure_extension(path)?;

    let file = File::open(path).with_context(|| path.display().to_string())?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("{} is not a valid Darkroom archive", path.display()))?;
    let has_overlapping_files = archive
        .has_overlapping_files()
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    ensure!(
        !has_overlapping_files,
        "{} contains overlapping ZIP entries",
        path.display()
    );

    let document_entries = archive
        .file_names()
        .filter(|name| *name == DOCUMENT_ENTRY)
        .count();
    ensure!(
        document_entries == 1,
        "{} must contain exactly one {DOCUMENT_ENTRY}",
        path.display()
    );

    let mut entry = archive
        .by_name(DOCUMENT_ENTRY)
        .with_context(|| format!("failed to open {DOCUMENT_ENTRY} in {}", path.display()))?;
    ensure!(
        entry.is_file(),
        "{} contains a non-file {DOCUMENT_ENTRY} entry",
        path.display()
    );
    ensure_document_size(entry.size())?;

    let mut json = Vec::with_capacity(entry.size() as usize);
    (&mut entry)
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut json)
        .with_context(|| format!("failed to read {DOCUMENT_ENTRY} from {}", path.display()))?;
    ensure_document_size(json.len() as u64)?;

    let document: Document = serde_json::from_slice(&json)
        .with_context(|| format!("invalid {DOCUMENT_ENTRY} in {}", path.display()))?;
    document
        .validate()
        .with_context(|| path.display().to_string())?;
    Ok(document)
}

pub(crate) fn save(document: &Document, path: &Path) -> Result<()> {
    ensure_extension(path)?;
    document.validate_debug();

    let json = serde_json::to_vec_pretty(document).with_context(|| {
        format!(
            "failed to serialize {DOCUMENT_ENTRY} for {}",
            path.display()
        )
    })?;
    ensure_document_size(json.len() as u64)?;

    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        write_archive(file, &json)
    })
    .with_context(|| path.display().to_string())
}

pub(crate) fn with_extension(mut path: PathBuf) -> PathBuf {
    if !has_extension(&path) {
        path.set_extension(EXTENSION);
    }
    path
}

fn ensure_extension(path: &Path) -> Result<()> {
    ensure!(
        has_extension(path),
        "Darkroom projects must use the .{EXTENSION} extension"
    );
    Ok(())
}

fn has_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION))
}

fn ensure_document_size(size: u64) -> Result<()> {
    ensure!(
        size <= MAX_DOCUMENT_BYTES,
        "{DOCUMENT_ENTRY} exceeds the {} MiB size limit",
        MAX_DOCUMENT_BYTES / (1024 * 1024)
    );
    Ok(())
}

fn write_archive(file: &mut File, json: &[u8]) -> io::Result<()> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut archive = ZipWriter::new(file);
    archive
        .start_file(DOCUMENT_ENTRY, options)
        .map_err(io::Error::other)?;
    archive.write_all(json)?;
    archive.finish().map_err(io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use common::test_utils::test_output_path;
    use scenarium::GraphId;

    use super::*;

    #[test]
    fn project_round_trips_as_one_json_entry() {
        let path = test_output_path("darkroom_project/roundtrip.darkroom");
        let document = Document::default();

        save(&document, &path).expect("save project");
        assert_eq!(load(&path).expect("load project"), document);

        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        assert_eq!(archive.len(), 1, "projects contain only the document entry");
        let mut entry = archive.by_name(DOCUMENT_ENTRY).unwrap();
        assert_eq!(entry.compression(), CompressionMethod::Deflated);
        let mut json = String::new();
        entry.read_to_string(&mut json).unwrap();
        let decoded: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, document, "the archive payload is plain JSON");
    }

    #[test]
    fn project_extension_is_required_case_insensitively() {
        let document = Document::default();
        let wrong = test_output_path("darkroom_project/wrong.json");
        let error = save(&document, &wrong).unwrap_err().to_string();
        assert!(error.contains(".darkroom extension"), "{error}");

        let uppercase = test_output_path("darkroom_project/uppercase.DARKROOM");
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
    fn load_rejects_invalid_archives_and_missing_or_invalid_documents() {
        let corrupt = test_output_path("darkroom_project/corrupt.darkroom");
        std::fs::write(&corrupt, b"not a zip archive").unwrap();
        let error = format!("{:#}", load(&corrupt).unwrap_err());
        assert!(error.contains("not a valid Darkroom archive"), "{error}");

        let missing = test_output_path("darkroom_project/missing.darkroom");
        write_test_archive(&missing, "other.json", b"{}");
        let error = format!("{:#}", load(&missing).unwrap_err());
        assert!(error.contains("exactly one project.json"), "{error}");

        let malformed = test_output_path("darkroom_project/malformed.darkroom");
        write_test_archive(&malformed, DOCUMENT_ENTRY, b"{");
        let error = format!("{:#}", load(&malformed).unwrap_err());
        assert!(error.contains("invalid project.json"), "{error}");

        let invalid = test_output_path("darkroom_project/invalid.darkroom");
        let mut document = Document::default();
        document.graph.origin = Some(GraphId::nil());
        let json = serde_json::to_vec(&document).unwrap();
        write_test_archive(&invalid, DOCUMENT_ENTRY, &json);
        let error = format!("{:#}", load(&invalid).unwrap_err());
        assert!(error.contains("graph has a nil origin"), "{error}");
    }

    #[test]
    fn document_size_limit_rejects_the_first_byte_over_the_boundary() {
        ensure_document_size(MAX_DOCUMENT_BYTES).expect("boundary is accepted");
        let error = ensure_document_size(MAX_DOCUMENT_BYTES + 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains("256 MiB size limit"), "{error}");
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
}
