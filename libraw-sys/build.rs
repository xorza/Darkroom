use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// One list for both the compiled library and the parsed headers, so the two cannot disagree.
/// Static only, so `LIBRAW_NODLL` everywhere. Thread-safe (no `LIBRAW_NOTHREADS`): frames decode
/// on concurrent rayon workers, each with its own `libraw_data_t`.
const DEFINES: &[&str] = &["LIBRAW_NODLL"];

fn main() {
    let root = Path::new("LibRaw");
    compile(root);
    generate_bindings(root);

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=shim");
    for dir in ["src", "libraw", "internal"] {
        println!("cargo:rerun-if-changed={}", root.join(dir).display());
    }
}

fn compile(root: &Path) {
    let mut sources = Vec::new();
    collect_sources(&root.join("src"), &mut sources);
    assert!(
        !sources.is_empty(),
        "no LibRaw sources under {}: run `git submodule update --init libraw-sys/LibRaw`",
        root.display()
    );
    sources.sort();
    // Not part of LibRaw: the accessors for state its C API does not expose. Compiled with it, so
    // it sees the same headers and defines.
    sources.push(PathBuf::from("shim/internal.cpp"));

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .include(root)
        .files(&sources)
        .flag_if_supported("-pthread")
        .warnings(false)
        .extra_warnings(false);
    for define in DEFINES {
        build.define(define, None);
    }
    build.compile("raw");
}

/// Bindings for the target being built: bindgen hands Cargo's `TARGET` to clang, so the struct
/// layouts, and the layout assertions bindgen emits, are that target's own.
fn generate_bindings(root: &Path) {
    let header = root.join("libraw/libraw.h");
    bindgen::Builder::default()
        .header(header.to_str().expect("UTF-8 crate path"))
        .clang_args(DEFINES.iter().map(|define| format!("-D{define}")))
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .use_core()
        .ctypes_prefix("core::ffi")
        .allowlist_function("libraw_.*")
        .allowlist_type("libraw_.*")
        .allowlist_type("LibRaw_errors")
        .allowlist_var("LIBRAW_.*")
        .derive_debug(true)
        .derive_default(true)
        .generate_comments(false)
        .formatter(bindgen::Formatter::Prettyplease)
        .generate()
        .expect("LibRaw headers parse; bindgen needs libclang, see the root AGENTS.md")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("bindings.rs"))
        .expect("writable OUT_DIR");
}

/// Every translation unit of the library, which is every `.cpp` under `src/` except the `*_ph.cpp`
/// placeholders: those stub out the processing stages for stripped builds and define the same
/// symbols as the real ones.
fn collect_sources(dir: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("readable LibRaw source directory") {
        let path = entry.expect("readable directory entry").path();
        if path.is_dir() {
            collect_sources(&path, sources);
        } else if path.extension().is_some_and(|ext| ext == "cpp")
            && !path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.ends_with("_ph"))
        {
            sources.push(path);
        }
    }
}
