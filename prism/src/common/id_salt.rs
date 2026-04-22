//! Standardized ID salt generation for egui persistent IDs.
//!
//! # The [`StableId`] type
//!
//! Every widget that needs a stable identity across frames should use
//! [`StableId`]. It is the *only* type our own scope primitives
//! (`Gui::scoped`, `Button::show`, `Frame::show`, etc.) accept — that's
//! how the compiler enforces "your id came from an approved source."
//!
//! Approved sources are:
//! - [`StableId::new`] — the common case. `#[track_caller]` mixes the
//!   call site's `file!()`/`line!()` into the hash so two buttons with
//!   the same name at different locations don't collide.
//! - [`StableId::from_id`] — escape hatch when you already have an
//!   [`egui::Id`] (e.g. inherited from a caller).
//!
//! For list items or per-instance widgets, pass a tuple that includes
//! the runtime key:
//!
//! ```ignore
//! Button::default().show(gui, StableId::new("cache"));
//! Button::default().show(gui, StableId::new(("cache_btn", node.id)));
//! Button::default().show(gui, StableId::new(("func_btn", func.id)));
//! ```

use std::hash::Hash;

use egui::Id;

/// A widget id pinned to the call site where it's constructed.
///
/// The wrapped [`Id`] is used verbatim as a child Ui's registered widget
/// id via `UiBuilder::id(...)` (global_scope=true) — bypassing egui's
/// default `unique_id = stable_id.with(parent_counter)` formula, which
/// drifts whenever conditional siblings come and go in the parent Ui
/// and triggers "widget rect changed id between passes" warnings on
/// our fixed-rect chrome.
///
/// Constructing a `StableId` from a raw [`Id`] or arbitrary hashable
/// value requires going through [`StableId::new`] (call-site salted)
/// or [`StableId::from_id`] (explicit). The type makes the "where did
/// this id come from?" question answerable by the compiler.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StableId(Id);

impl StableId {
    /// Build an id rooted at the call site (`#[track_caller]`) plus
    /// the caller-supplied `name`. The name can be a literal string, a
    /// runtime key, or a tuple composing both (the common list-item
    /// pattern: `StableId::new(("func_btn", func.id))`).
    #[track_caller]
    pub fn new(name: impl Hash) -> Self {
        let loc = std::panic::Location::caller();
        Self(Id::new((loc.file(), loc.line(), name)))
    }

    /// Wrap an already-constructed [`Id`]. Use this when the id is
    /// inherited from a caller (e.g. `PositionedUi::new(id, ...)`) and
    /// rehashing with `StableId::new` would split the two ids apart.
    pub fn from_id(id: Id) -> Self {
        Self(id)
    }

    /// Access the underlying [`Id`]. Needed for interop with egui APIs
    /// that don't flow through our wrappers (e.g. `UiBuilder::id`
    /// directly, or `make_persistent_id` call sites).
    pub fn id(self) -> Id {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Egui chrome types app code must not import. POD types (`Rect`,
    /// `Vec2`, `Color32`, `Sense`, `Key`, `Response`, ...) stay
    /// allowed — see `prism/EGUI_ENCAPSULATION_PLAN.md` non-goals.
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
        let crate_root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let whitelist: &[&Path] = &[
            // Sanctioned home of the helper definitions:
            Path::new("gui/mod.rs"),
            // This test file references the literal patterns in its
            // own scanner code.
            Path::new("common/id_salt.rs"),
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
        const PATTERNS: &[&str] = &[
            "UiBuilder::new(",
            ".allocate_rect(",
            ".allocate_exact_size(",
            ".allocate_space(",
            ".scope_builder(",
        ];

        let mut offenders = Vec::new();
        visit(&crate_root, &crate_root, whitelist, &mut offenders);

        assert!(
            offenders.is_empty(),
            "Found drifting widget-id pattern. Use `Gui::scope` (and \
             allocate inside the scope) instead, or annotate the line with \
             `// id-drift-ok` if intentional. Call sites:\n{}",
            offenders.join("\n"),
        );

        fn visit(dir: &Path, root: &Path, whitelist: &[&Path], offenders: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, root, whitelist, offenders);
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
                    let Some(matched) = PATTERNS.iter().find(|p| line.contains(*p)) else {
                        continue;
                    };
                    // Allow whitelist on the same line OR up to two
                    // preceding non-blank lines.
                    let same_line = line.contains("// id-drift-ok");
                    let preceding_ok = lines[..lineno]
                        .iter()
                        .rev()
                        .filter(|l| !l.trim().is_empty())
                        .take(2)
                        .any(|l| l.contains("// id-drift-ok"));
                    if !same_line && !preceding_ok {
                        offenders.push(format!(
                            "{}:{}: [{}] {}",
                            path.display(),
                            lineno + 1,
                            matched,
                            trimmed
                        ));
                    }
                }
            }
        }
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
        let crate_root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // Files outside `gui/widgets/` that legitimately hold raw egui
        // access. Keep this set small and justified.
        let whitelist: &[&Path] = &[
            // The accessor itself + doc mentions.
            Path::new("gui/mod.rs"),
            Path::new("common/id_salt.rs"),
        ];

        let mut offenders = Vec::new();
        visit(&crate_root, &crate_root, whitelist, &mut offenders);

        assert!(
            offenders.is_empty(),
            "Found `gui.ui_raw()` outside `gui/widgets/`. Build a widget \
             instead, or annotate with `// egui-direct-ok` if intentional. \
             Call sites:\n{}",
            offenders.join("\n"),
        );

        fn visit(dir: &Path, root: &Path, whitelist: &[&Path], offenders: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Skip the whole widgets subtree — that's where
                    // `ui_raw()` is supposed to live.
                    if path.ends_with("gui/widgets") {
                        continue;
                    }
                    visit(&path, root, whitelist, offenders);
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
                    if !line.contains("ui_raw()") {
                        continue;
                    }
                    let same_line = line.contains("// egui-direct-ok");
                    let preceding_ok = lines[..lineno]
                        .iter()
                        .rev()
                        .filter(|l| !l.trim().is_empty())
                        .take(2)
                        .any(|l| l.contains("// egui-direct-ok"));
                    if !same_line && !preceding_ok {
                        offenders.push(format!("{}:{}: {}", path.display(), lineno + 1, trimmed));
                    }
                }
            }
        }
    }

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
        let crate_root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let whitelist: &[&Path] = &[
            // Root eframe integration — wraps the top-level egui::Ui
            // and drives egui::CentralPanel / egui::Frame / etc. for
            // the application shell.
            Path::new("main_ui.rs"),
            Path::new("main.rs"),
            // Defines the `Gui` wrapper over `egui::Ui`; necessarily
            // imports `Ui` and `UiBuilder`.
            Path::new("gui/mod.rs"),
            // Scanner defines the banned names as literals.
            Path::new("common/id_salt.rs"),
        ];

        let mut offenders = Vec::new();
        visit(&crate_root, &crate_root, whitelist, &mut offenders);

        assert!(
            offenders.is_empty(),
            "Found `use egui::<chrome>` import outside `gui/widgets/`. \
             Use the `widgets::` shadow (e.g. `widgets::Frame`), or annotate \
             with `// egui-chrome-ok` if intentional. Banned names: {:?}. \
             Call sites:\n{}",
            EGUI_CHROME_TYPES,
            offenders.join("\n"),
        );

        fn visit(dir: &Path, root: &Path, whitelist: &[&Path], offenders: &mut Vec<String>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.ends_with("gui/widgets") {
                        continue;
                    }
                    visit(&path, root, whitelist, offenders);
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
                    // Only scan `use` lines — chrome names appearing as
                    // qualified paths (`egui::Ui::foo`) are rare and,
                    // where they do appear, they still need an import
                    // elsewhere which this test catches.
                    if !trimmed.starts_with("use egui") && !trimmed.starts_with("pub use egui") {
                        continue;
                    }
                    let banned = EGUI_CHROME_TYPES
                        .iter()
                        .find(|name| contains_ident(line, name));
                    let Some(&matched) = banned else {
                        continue;
                    };
                    let same_line = line.contains("// egui-chrome-ok");
                    let preceding_ok = lines[..lineno]
                        .iter()
                        .rev()
                        .filter(|l| !l.trim().is_empty())
                        .take(2)
                        .any(|l| l.contains("// egui-chrome-ok"));
                    if !same_line && !preceding_ok {
                        offenders.push(format!(
                            "{}:{}: [{}] {}",
                            path.display(),
                            lineno + 1,
                            matched,
                            trimmed
                        ));
                    }
                }
            }
        }

        /// True iff `name` appears in `line` as a whole-word identifier,
        /// not as a substring of a longer name (so `Ui` doesn't match
        /// `UiBuilder`, `Frame` doesn't match `FramePainter`, etc.).
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
    }
}
