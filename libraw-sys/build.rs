use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// One list for both the compiled library and the parsed headers, so the two cannot disagree.
/// Static only, so `LIBRAW_NODLL` everywhere. Thread-safe (no `LIBRAW_NOTHREADS`): frames decode
/// on concurrent rayon workers, each with its own `libraw_data_t`. `USE_ZLIB` for deflate DNG, as
/// LibRaw's own `Makefile.dist` builds it, against the zlib `libz-sys` builds.
const DEFINES: &[&str] = &["LIBRAW_NODLL", "USE_ZLIB"];

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
    build.include(zlib_include());
    // LibRaw parallelizes its tiled decoders — Canon CR3, compressed Fuji, Panasonic v8 — under
    // OpenMP alone, which it enables wherever the compiler defines `_OPENMP`.
    let openmp = openmp(&build);
    match &openmp {
        Some(openmp) => {
            for flag in &openmp.flags {
                build.flag(flag);
            }
        }
        None => println!(
            "cargo:warning=no OpenMP runtime links with this compiler: LibRaw decodes CR3, \
             compressed Fuji and Panasonic v8 frames on one thread each"
        ),
    }
    build.compile("raw");
    println!("cargo::rustc-check-cfg=cfg(libraw_openmp)");
    // After the library, so the linker resolves the runtime the library calls.
    if let Some(openmp) = openmp {
        println!("cargo::rustc-cfg=libraw_openmp");
        if let Some(search) = &openmp.search {
            println!("cargo:rustc-link-search=native={}", search.display());
        }
        if let Some(library) = openmp.library {
            println!("cargo:rustc-link-lib={library}");
        }
    }
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

/// How this toolchain compiles OpenMP, and the runtime the final link needs for it.
#[derive(Debug)]
struct OpenMp {
    /// For every translation unit.
    flags: Vec<String>,
    /// The runtime rustc links; `None` where the compiler names it in the objects itself, as
    /// MSVC does.
    library: Option<&'static str>,
    /// Where the runtime is, when not on the linker's own path.
    search: Option<PathBuf>,
}

/// OpenMP for `build`'s compiler, or `None` when it cannot compile and link a program using it:
/// GCC's `-fopenmp` and libgomp, Clang's and libomp, MSVC's `/openmp`, and on macOS, whose Clang
/// ships no runtime, Homebrew's libomp.
fn openmp(build: &cc::Build) -> Option<OpenMp> {
    let compiler = build.get_compiler();
    let openmp = if compiler.is_like_msvc() {
        OpenMp {
            flags: vec!["/openmp".to_owned()],
            library: None,
            search: None,
        }
    } else if env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "macos") {
        let prefix = ["/opt/homebrew/opt/libomp", "/usr/local/opt/libomp"]
            .into_iter()
            .map(PathBuf::from)
            .find(|prefix| prefix.join("include/omp.h").is_file())?;
        OpenMp {
            flags: vec![
                "-Xpreprocessor".to_owned(),
                "-fopenmp".to_owned(),
                format!("-I{}", prefix.join("include").display()),
            ],
            library: Some("omp"),
            search: Some(prefix.join("lib")),
        }
    } else {
        OpenMp {
            flags: vec!["-fopenmp".to_owned()],
            library: Some(if compiler.is_like_clang() {
                "omp"
            } else {
                "gomp"
            }),
            search: None,
        }
    };
    openmp.links(&compiler).then_some(openmp)
}

impl OpenMp {
    /// Whether `compiler` compiles and links a program calling the runtime under these flags: the
    /// flag alone can pass where the runtime is missing, and the link would then fail later.
    fn links(&self, compiler: &cc::Tool) -> bool {
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        let probe = out.join("openmp_probe.cpp");
        fs::write(
            &probe,
            "#include <omp.h>\nint main() { return omp_get_max_threads() > 0 ? 0 : 1; }\n",
        )
        .expect("writable OUT_DIR");
        let mut command = compiler.to_command();
        command.current_dir(&out).args(&self.flags).arg(&probe);
        if compiler.is_like_msvc() {
            command.arg("/Feopenmp_probe.exe");
        } else {
            command.arg("-o").arg(out.join("openmp_probe"));
            if let Some(search) = &self.search {
                command.arg(format!("-L{}", search.display()));
            }
            if let Some(library) = self.library {
                command.arg(format!("-l{library}"));
            }
        }
        command.output().is_ok_and(|output| output.status.success())
    }
}

/// The headers of the zlib `libz-sys` built, which it names through its `links = "z"`.
fn zlib_include() -> PathBuf {
    PathBuf::from(env::var_os("DEP_Z_INCLUDE").expect("libz-sys states its include directory"))
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
