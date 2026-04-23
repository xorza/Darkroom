//! Crate-wide discipline tripwires: cheap `grep`-style scanners that run
//! as `#[test]` functions and fail CI if a banned pattern appears outside
//! a narrow whitelist. Each tripwire documents its own annotation comment
//! (`// id-drift-ok`, `// egui-direct-ok`, `// egui-chrome-ok`) for
//! intentional exceptions, honoured on the same line or up to two
//! preceding non-blank lines.

#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};

/// Walks the crate's `src/` tree and accumulates offender lines.
///
/// `check_line` returns `Some(info)` for a matching line — `info` is the
/// human-readable tail of the offender message (e.g. `"[pattern] <trimmed>"`).
/// The walker handles path filtering, whitelist, comment skip, and the
/// annotation-exemption window.
fn scan_crate(
    whitelist: &[&Path],
    skip_dirs: &[&Path],
    annotation: &str,
    mut check_line: impl FnMut(&str) -> Option<String>,
) -> Vec<String> {
    let crate_root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    visit(
        &crate_root,
        &crate_root,
        whitelist,
        skip_dirs,
        annotation,
        &mut check_line,
        &mut offenders,
    );
    return offenders;

    fn visit(
        dir: &Path,
        root: &Path,
        whitelist: &[&Path],
        skip_dirs: &[&Path],
        annotation: &str,
        check_line: &mut dyn FnMut(&str) -> Option<String>,
        offenders: &mut Vec<String>,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                if skip_dirs.iter().any(|d| rel.ends_with(d)) {
                    continue;
                }
                visit(
                    &path, root, whitelist, skip_dirs, annotation, check_line, offenders,
                );
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path);
            if whitelist.contains(&rel) {
                continue;
            }
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            let lines: Vec<&str> = contents.lines().collect();
            for (lineno, line) in lines.iter().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") || trimmed.starts_with('*') {
                    continue;
                }
                let Some(info) = check_line(line) else {
                    continue;
                };
                let same_line = line.contains(annotation);
                let preceding_ok = lines[..lineno]
                    .iter()
                    .rev()
                    .filter(|l| !l.trim().is_empty())
                    .take(2)
                    .any(|l| l.contains(annotation));
                if !same_line && !preceding_ok {
                    offenders.push(format!("{}:{}: {}", path.display(), lineno + 1, info));
                }
            }
        }
    }
}

/// Tripwire for the egui widget-id drift bug.
///
/// Flags any of these patterns outside whitelisted files:
///
/// - `UiBuilder::new(` — the underlying drift culprit (its `id_salt`
///   produces `unique_id = stable_id.with(parent_counter)`, see
///   `egui-0.34.1/src/ui.rs:297`). Use `Gui::scope` instead.
/// - `.allocate_rect(`, `.allocate_exact_size(`, `.allocate_space(`
///   — these emit a counter-based auto-id widget that drifts the
///   same way. Wrap the call in a `Gui::scope` first so the
///   scope's stable seed gives a stable counter=0 auto-id.
/// - `.scope_builder(` — egui's raw scope API. Use our
///   `Gui::scope` so we pass a `StableId` and `global_scope`.
///
/// Annotate intentional exceptions with `// id-drift-ok` on the
/// same line or up to two non-blank lines above (e.g. when working
/// with raw `egui::Ui`, not our `Gui`).
#[test]
fn no_drifting_widget_ids_in_crate() {
    const PATTERNS: &[&str] = &[
        "UiBuilder::new(",
        ".allocate_rect(",
        ".allocate_exact_size(",
        ".allocate_space(",
        ".scope_builder(",
    ];
    let whitelist: &[&Path] = &[
        // Sanctioned home of the helper definitions:
        Path::new("gui/mod.rs"),
        // This file defines the banned patterns as string literals.
        Path::new("common/tripwires.rs"),
        // Allocates inside an already-scoped child Ui — the
        // canonical pattern this whole test exists to enforce.
        Path::new("gui/widgets/button.rs"),
        // Fork of egui's TextEdit; its allocations live inside
        // egui's own widget-id discipline, configured per-call
        // via `.id_salt(StableId)` on the public API.
        Path::new("gui/widgets/text_edit.rs"),
        // Expander uses a stable `Id::new(text)` and `interact`
        // with that id directly. The auto-id from allocate_space
        // is discarded; left as-is for now.
        Path::new("gui/widgets/expander.rs"),
    ];

    let offenders = scan_crate(whitelist, &[], "// id-drift-ok", |line| {
        PATTERNS
            .iter()
            .find(|p| line.contains(*p))
            .map(|p| format!("[{}] {}", p, line.trim_start()))
    });

    assert!(
        offenders.is_empty(),
        "Found drifting widget-id pattern. Use `Gui::scope` (and \
         allocate inside the scope) instead, or annotate the line with \
         `// id-drift-ok` if intentional. Call sites:\n{}",
        offenders.join("\n"),
    );
}

/// Tripwire for direct egui access outside the wrapper layer.
///
/// App code must not call `gui.ui_raw()` — every interaction with
/// `egui::Ui` / `egui::Context` goes through a widget in
/// `gui/widgets/`. The `ui_raw()` accessor only exists so widgets
/// can talk to the underlying egui; it is not part of the app API.
///
/// Annotate intentional exceptions with `// egui-direct-ok` on the
/// same line or up to two non-blank lines above.
#[test]
fn no_raw_ui_outside_widgets() {
    let whitelist: &[&Path] = &[
        // The accessor itself + doc mentions.
        Path::new("gui/mod.rs"),
        // This file matches its own `ui_raw()` substring in the scanner.
        Path::new("common/tripwires.rs"),
        // eframe boundary: wraps the root `egui::Ui` handed to
        // `impl App::ui` and drives egui::Panel / MenuBar that
        // need `&mut egui::Ui` arguments. Downstream of the
        // panels everything is Gui.
        Path::new("main_gui.rs"),
    ];
    let skip_dirs: &[&Path] = &[Path::new("gui/widgets")];

    let offenders = scan_crate(whitelist, skip_dirs, "// egui-direct-ok", |line| {
        line.contains("ui_raw()")
            .then(|| line.trim_start().to_string())
    });

    assert!(
        offenders.is_empty(),
        "Found `gui.ui_raw()` outside `gui/widgets/`. Build a widget \
         instead, or annotate with `// egui-direct-ok` if intentional. \
         Call sites:\n{}",
        offenders.join("\n"),
    );
}

/// Egui chrome types app code must not import. POD types (`Rect`,
/// `Vec2`, `Color32`, `Sense`, `Key`, `Response`, ...) stay allowed —
/// see `prism/EGUI_ENCAPSULATION_PLAN.md` non-goals.
const EGUI_CHROME_TYPES: &[&str] = &[
    "UiBuilder",
    "Ui",
    "Frame",
    "ScrollArea",
    "Window",
    "Area",
    "Panel",
    "CentralPanel",
    "SidePanel",
    "TopBottomPanel",
    "CollapsingHeader",
    "CollapsingState",
    "ComboBox",
    "Button",
    "TextEdit",
    "collapsing_header",
];

/// Tripwire for chrome-type imports outside the wrapper layer.
///
/// `use egui::Frame;` (and friends) in app code bypasses the
/// widget wrapper and reintroduces the coupling we spent the plan
/// eliminating. Each banned name in `EGUI_CHROME_TYPES` has a
/// `widgets::<same-name>` shadow; use that.
///
/// POD types (`Rect`, `Vec2`, `Color32`, `Sense`, `Key`,
/// `Response`, ...) are explicitly non-goals and stay allowed.
///
/// Annotate intentional exceptions with `// egui-chrome-ok` on the
/// same line or up to two non-blank lines above.
#[test]
fn no_egui_chrome_outside_widgets() {
    let whitelist: &[&Path] = &[
        // Root eframe integration — wraps the top-level egui::Ui
        // and drives egui::CentralPanel / egui::Frame / etc. for
        // the application shell.
        Path::new("main_gui.rs"),
        Path::new("main.rs"),
        // Defines the `Gui` wrapper over `egui::Ui`; necessarily
        // imports `Ui` and `UiBuilder`.
        Path::new("gui/mod.rs"),
        // Scanner defines the banned names as string literals.
        Path::new("common/tripwires.rs"),
    ];
    let skip_dirs: &[&Path] = &[Path::new("gui/widgets")];

    let offenders = scan_crate(whitelist, skip_dirs, "// egui-chrome-ok", |line| {
        let trimmed = line.trim_start();
        // Only scan `use` lines — chrome names appearing as
        // qualified paths (`egui::Ui::foo`) are rare and, where
        // they do appear, they still need an import elsewhere which
        // this test catches.
        if !trimmed.starts_with("use egui") && !trimmed.starts_with("pub use egui") {
            return None;
        }
        EGUI_CHROME_TYPES
            .iter()
            .find(|name| contains_ident(line, name))
            .map(|name| format!("[{}] {}", name, trimmed))
    });

    assert!(
        offenders.is_empty(),
        "Found `use egui::<chrome>` import outside `gui/widgets/`. \
         Use the `widgets::` shadow (e.g. `widgets::Frame`), or annotate \
         with `// egui-chrome-ok` if intentional. Banned names: {:?}. \
         Call sites:\n{}",
        EGUI_CHROME_TYPES,
        offenders.join("\n"),
    );
}

/// True iff `name` appears in `line` as a whole-word identifier, not
/// as a substring of a longer name (so `Ui` doesn't match `UiBuilder`,
/// `Frame` doesn't match `FramePainter`, etc.).
fn contains_ident(line: &str, name: &str) -> bool {
    let bytes = line.as_bytes();
    let nb = name.as_bytes();
    let mut i = 0;
    while let Some(off) = line[i..].find(name) {
        let start = i + off;
        let end = start + nb.len();
        let before_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        i = end;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
