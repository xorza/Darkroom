use std::path::PathBuf;
use std::sync::OnceLock;

/// Returns the workspace root directory.
/// Works by finding the directory containing Cargo.lock.
fn workspace_root() -> PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest_dir).parent().unwrap().to_path_buf()
}

/// Ensures the test output directory exists. Safe to call multiple times.
pub fn ensure_test_output_dir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        std::fs::create_dir_all(workspace_root().join("test_output"))
            .expect("Failed to create test_output directory");
    });
}

/// Returns the path to a test output file.
pub fn test_output_path(name: &str) -> PathBuf {
    ensure_test_output_dir();
    workspace_root().join("test_output").join(name)
}
