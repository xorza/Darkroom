//! Embed the Windows application icon into the executable, and record the commit the binary is
//! built from.
//!
//! For a Windows target, compiles `darkroom/assets/icons/darkroom.ico` into the `.exe` as a Win32
//! resource so Explorer, the taskbar, and Alt-Tab show the app icon. The target is read from
//! `CARGO_CFG_TARGET_OS`: a build script runs on the host, so its own `cfg(windows)` is the
//! host's, as winresource's README warns. A build from another host needs the resource compiler
//! winresource names: `llvm-rc` for MSVC, MinGW's `windres` for GNU. The in-window title-bar icon
//! is set separately at runtime — see `load_icon` in `darkroom/src/lib.rs`.
//!
//! The commit, and whether the tree differed from it, lets `darkroom_open` say when a running
//! server is older than the source. The script reruns when the checkout moves or any workspace
//! source changes, so the record follows an edit as the binary does.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "a build script runs git, reads cargo's environment and reads the workspace at build \
              time, and `common` is not one of its dependencies"
)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ICON: &str = "../darkroom/assets/icons/darkroom.ico";

fn main() {
    embed_icon();
    record_build();
}

fn embed_icon() {
    println!("cargo:rerun-if-changed={ICON}");
    if env::var("CARGO_CFG_TARGET_OS").expect("cargo states the target OS") != "windows" {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ICON);
    res.compile()
        .expect("embed darkroom.ico into the Windows executable");
}

fn record_build() {
    let commit = git(&["rev-parse", "--short=12", "HEAD"]);
    let build = match (&commit, git(&["status", "--porcelain"])) {
        (Some(commit), Some(status)) if status.is_empty() => commit.clone(),
        (Some(commit), Some(_)) => format!("{commit} with uncommitted changes"),
        _ => "an unknown commit".to_string(),
    };
    println!("cargo:rustc-env=DARKROOM_BUILD={build}");
    if let Some(git_dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
    }
    for source in sources(Path::new("..")) {
        println!("cargo:rerun-if-changed={}", source.display());
    }
}

/// What the binary is built from: the root manifest and lockfile, and everything in each crate
/// folder at the root, members and submodules alike, sources and the assets they include. Found
/// rather than listed, so a new crate is watched without an edit here; a `target` folder, which
/// every build changes, is never among them.
fn sources(root: &Path) -> Vec<PathBuf> {
    let mut sources = vec![root.join("Cargo.toml"), root.join("Cargo.lock")];
    for folder in entries(root) {
        if folder.join("Cargo.toml").is_file() {
            sources.extend(entries(&folder));
        }
    }
    sources
}

/// The entries of `dir`, but for `target` and the hidden ones.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name != "target" && !name.starts_with('.')
        })
        .map(|entry| entry.path())
        .collect()
}

/// What git prints for `args`, trimmed; `None` where git is missing or the tree is no checkout.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
