//! File utility functions for listing and filtering files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Returns sorted paths to all files in a directory matching the given extensions.
///
/// Extensions are matched case-insensitively. Directory, entry, and metadata
/// errors are returned with the affected path.
pub fn files_with_extensions(dir: &Path, extensions: &[&str]) -> io::Result<Vec<PathBuf>> {
    let entries = fs::read_dir(dir).map_err(|error| {
        path_error(
            format!("failed to read directory '{}'", dir.display()),
            error,
        )
    })?;
    let mut files = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| {
            path_error(
                format!("failed to read entry in directory '{}'", dir.display()),
                error,
            )
        })?;
        let path = entry.path();
        let metadata = fs::metadata(&path).map_err(|error| {
            path_error(
                format!("failed to read metadata for '{}'", path.display()),
                error,
            )
        })?;
        if !metadata.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if extensions
            .iter()
            .any(|expected| extension.eq_ignore_ascii_case(expected))
        {
            files.push(path);
        }
    }

    files.sort();
    Ok(files)
}

fn path_error(message: String, source: io::Error) -> io::Error {
    io::Error::new(source.kind(), format!("{message}: {source}"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::PathBuf;

    use crate::file_utils::files_with_extensions;
    use crate::test_utils::test_output_path;

    fn fixture_dir(name: &str) -> PathBuf {
        let dir = test_output_path(&format!("common/file_utils/{name}"));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("remove stale file-utils fixture");
        }
        fs::create_dir_all(&dir).expect("create file-utils fixture");
        dir
    }

    fn names(paths: &[PathBuf]) -> Vec<&str> {
        paths
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect()
    }

    #[test]
    fn populated_directory_is_filtered_case_insensitively_and_sorted() {
        let dir = fixture_dir("populated");
        fs::write(dir.join("z.raf"), []).unwrap();
        fs::write(dir.join("a.RAF"), []).unwrap();
        fs::write(dir.join("ignored.fit"), []).unwrap();
        fs::create_dir(dir.join("nested.raf")).unwrap();

        let files = files_with_extensions(&dir, &["raf"]).unwrap();

        assert_eq!(names(&files), ["a.RAF", "z.raf"]);
        assert!(files.iter().all(|path| path.is_file()));
    }

    #[test]
    fn readable_empty_directory_is_distinct_from_scan_failure() {
        let dir = fixture_dir("empty");
        assert_eq!(
            files_with_extensions(&dir, &["raf"]).unwrap(),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn missing_directory_returns_contextual_error() {
        let dir = fixture_dir("missing");
        fs::remove_dir(&dir).unwrap();

        let error = files_with_extensions(&dir, &["raf"]).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains(&dir.display().to_string()));
    }

    #[test]
    fn file_instead_of_directory_returns_contextual_error() {
        let dir = fixture_dir("not_directory");
        let path = dir.join("frame.raf");
        fs::write(&path, []).unwrap();

        let error = files_with_extensions(&path, &["raf"]).unwrap_err();

        assert!(error.to_string().contains(&path.display().to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_returns_contextual_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = fixture_dir("unreadable");
        let original = fs::metadata(&dir).unwrap().permissions();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0)).unwrap();
        let result = files_with_extensions(&dir, &["raf"]);
        fs::set_permissions(&dir, original).unwrap();

        let error = result.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains(&dir.display().to_string()));
    }
}
