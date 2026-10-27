use super::*;
use common::TempDir;

#[test]
fn cache_root_is_named_after_stem_beside_the_file() {
    // Absolute path: cache sits in the same dir, stem + `.darkroom-cache`.
    assert_eq!(
        root(Path::new("/proj/scene.darkroom")),
        PathBuf::from("/proj/scene.darkroom-cache")
    );
    // Relative path keeps its (empty) parent.
    assert_eq!(
        root(Path::new("scene.darkroom")),
        PathBuf::from("scene.darkroom-cache")
    );
    // No extension → the whole filename is the stem.
    assert_eq!(
        root(Path::new("/proj/scene")),
        PathBuf::from("/proj/scene.darkroom-cache")
    );
    // Two projects in one dir get distinct stores.
    assert_ne!(
        root(Path::new("/proj/a.darkroom")),
        root(Path::new("/proj/b.darkroom"))
    );
}

/// A prepare creates the root and its self-ignoring `.gitignore`, and says so
/// when it cannot: a root under a file is no folder to make.
#[test]
fn prepare_creates_dir_and_self_ignoring_gitignore() {
    let dir = TempDir::new("darkroom-cache");
    let document_path = dir.join("scene.darkroom");

    let root = root(&document_path);
    assert!(!fs::exists(&root), "naming the root creates nothing");
    prepare(&root).unwrap();

    assert_eq!(root, dir.join("scene.darkroom-cache"));
    assert!(fs::is_dir(&root), "cache dir created beside the document");
    let gitignore = root.join(".gitignore");
    assert_eq!(
        fs::read_to_string(&gitignore).unwrap(),
        "*\n",
        "the cache folder ignores its own contents"
    );

    let blocker = dir.join("blocker");
    fs::write(&blocker, b"x").unwrap();
    assert!(
        prepare(&blocker.join("cache")).is_err(),
        "a file is in the way"
    );
}
