use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::Path;

use common::{TempDir, fs};
use scenarium::{
    Binding, ConstValue, DataType, FileKindId, FileMode, FuncId, InputDecl, InputPort, internals,
    numbered_port,
};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::core::document::Document;
use crate::core::document::internals::DocumentFixture;
use crate::core::io::document::resources::{RefusedEntry, folder, unpack};
use crate::core::io::document::saved_paths::SavedPaths;
use crate::core::io::document::{load, save, save_bundling};

/// A document whose one node holds `path` on an input that reads a file; that input.
fn reading(path: &str) -> (Document, InputPort) {
    let reader = internals::stub_func(FuncId::unique(), "reader")
        .input(
            1,
            InputDecl::optional(
                "file",
                DataType::File {
                    kind: FileKindId::from_u128(1),
                    mode: FileMode::Open,
                },
            ),
        )
        .build();
    let mut fixture = DocumentFixture::default();
    let node = fixture.add(&reader);
    let port = InputPort::new(node, numbered_port(1));
    fixture
        .document
        .graphs
        .main_mut()
        .bindings
        .insert(port, Binding::Const(ConstValue::FsPath(path.to_string())));
    (fixture.document, port)
}

fn path_of(document: &Document, port: InputPort) -> String {
    match document.graphs.main().bindings.get(&port) {
        Some(Binding::Const(ConstValue::FsPath(path))) => path.clone(),
        other => panic!("{other:?} is no path"),
    }
}

fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// A file an export bundles goes into the archive under its digest; the open unpacks it beside
/// the archive, read-only, and the document names the unpacked file; a Save As to another
/// archive bundles it again, so the new archive opens alone, its source and the first archive's
/// folder gone; a plain text file bundles nothing.
#[test]
fn a_bundled_file_travels_in_the_archive_and_unpacks_beside_it() {
    let dir = TempDir::new("darkroom-resources");
    let source = dir.join("frame.fits");
    fs::write(&source, b"pixels").unwrap();
    let (document, port) = reading(source.to_str().unwrap());
    let bundle = BTreeSet::from([source.to_str().unwrap().to_string()]);
    let archive = dir.join("export").join("scene.darkroomz");
    fs::create_dir_all(archive.parent().unwrap()).unwrap();
    save_bundling(&document, &archive, SavedPaths::Relative, &bundle).unwrap();

    let loaded = load(&archive).unwrap();
    assert!(loaded.refused.is_empty(), "{:?}", loaded.refused);
    let unpacked = path_of(&loaded.document, port);
    // blake3 of "pixels" names the folder: the file's content, not its name.
    assert_eq!(
        Path::new(&unpacked),
        folder(&archive).join(digest(b"pixels")).join("frame.fits")
    );
    assert_eq!(fs::read(&unpacked).unwrap(), b"pixels");
    assert!(fs::metadata(&unpacked).unwrap().permissions().readonly());

    let again = dir.join("elsewhere.darkroomz");
    save(&loaded.document, &again, SavedPaths::Relative).unwrap();
    fs::remove_file(&source).unwrap();
    fs::remove_dir_all(folder(&archive)).unwrap();
    let reloaded = load(&again).unwrap();
    assert!(reloaded.refused.is_empty(), "{:?}", reloaded.refused);
    let moved = path_of(&reloaded.document, port);
    assert_eq!(
        Path::new(&moved),
        folder(&again).join(digest(b"pixels")).join("frame.fits")
    );
    assert_eq!(fs::read(moved).unwrap(), b"pixels");

    let plain = dir.join("plain.darkroom");
    assert!(save_bundling(&document, &plain, SavedPaths::Relative, &bundle).is_err());
}

/// An entry an archive holds is unpacked only when it is a whole, regular file under
/// `resources/` whose bytes match the digest its name gives: a mismatch, a missing entry and a
/// name past the form are each refused and leave the path as it was. An open removes a digest
/// folder the document no longer names and leaves any other folder there.
#[test]
fn an_entry_that_is_not_what_its_name_says_is_refused() {
    let dir = TempDir::new("darkroom-resources-refused");
    let archive = dir.join("scene.darkroomz");
    let good = format!("resources/{}/a.fits", digest(b"good"));
    let lying = format!("resources/{}/b.fits", digest(b"other"));
    let missing = format!("resources/{}/c.fits", digest(b"absent"));
    let mut zip = ZipWriter::new(fs::create(&archive).unwrap());
    for (name, bytes) in [(&good, &b"good"[..]), (&lying, &b"actual"[..])] {
        zip.start_file(name.as_str(), SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    let stale = folder(&archive).join(digest(b"stale"));
    let kept = folder(&archive).join("notes");
    fs::create_dir_all(&stale).unwrap();
    fs::create_dir_all(&kept).unwrap();

    let cases = [
        (good.clone(), true),
        (lying.clone(), false),
        (missing.clone(), false),
        ("resources/../escape".to_string(), false),
        (
            format!("resources/{}/a.fits", digest(b"good").to_uppercase()),
            false,
        ),
    ];
    for (name, unpacks) in cases {
        let (mut document, port) = reading(&name);
        let mut opened = ZipArchive::new(fs::open(&archive).unwrap()).unwrap();
        let refused = unpack(&mut opened, &archive, &mut document);
        assert_eq!(refused.len(), usize::from(!unpacks), "{name}: {refused:?}");
        assert_eq!(path_of(&document, port) != name, unpacks, "{name}");
        if let Some(refusal) = refused.first() {
            let expected = if name == lying {
                matches!(refusal, RefusedEntry::DigestMismatch { .. })
            } else if name == missing {
                matches!(refusal, RefusedEntry::Missing { .. })
            } else {
                matches!(refusal, RefusedEntry::Unnamed { .. })
            };
            assert!(expected, "{name}: {refusal:?}");
        }
    }
    assert!(!fs::exists(&stale), "a folder no entry names goes");
    assert!(fs::exists(&kept), "a folder of another name stays");

    // A file where the resources folder would go: the folder cannot be made, and the load
    // report names where.
    let blocked = dir.join("blocked.darkroomz");
    fs::copy(&archive, &blocked).unwrap();
    fs::write(folder(&blocked), b"in the way").unwrap();
    let (mut document, port) = reading(&good);
    let mut opened = ZipArchive::new(fs::open(&blocked).unwrap()).unwrap();
    let refused = unpack(&mut opened, &blocked, &mut document);
    assert!(
        matches!(refused.as_slice(), [RefusedEntry::Write { path, .. }]
            if *path == folder(&blocked).join(digest(b"good")).join("a.fits")),
        "{refused:?}"
    );
    assert_eq!(path_of(&document, port), good);
}
