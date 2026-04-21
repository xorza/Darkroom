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
/// value requires going through [`StableId::new`] (call-site salted),
/// [`StableId::from_id`] (explicit), or [`StableId::default`] (call-site
/// only). The type makes the "where did this id come from?" question
/// answerable by the compiler.
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

    /// Tripwire for the egui widget-id drift bug.
    ///
    /// Every `UiBuilder` we construct should go through `Gui::scoped_with`,
    /// which uses `.id(Id::new(salt))` (global_scope) — pinning the
    /// scope's widget id verbatim. Direct `UiBuilder::new()` chains
    /// risk silently using `.id_salt(...)`, whose `unique_id =
    /// stable_id.with(parent_counter)` drifts when conditional siblings
    /// come and go and trips egui's "widget rect changed id between
    /// passes" warning.
    ///
    /// This test fails on any bare `UiBuilder::new()` outside the
    /// whitelisted definition sites in `gui::Gui`. That forces a switch
    /// to `Gui::scoped_with`, or an explicit whitelist with `// id-drift-ok`
    /// justifying why direct UiBuilder use is intended.
    #[test]
    fn no_bare_ui_builder_in_crate() {
        let crate_root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // The two files that legitimately construct `UiBuilder` (the
        // `Gui::scoped_with` body lives in gui/mod.rs).
        let whitelist: &[&Path] = &[
            Path::new("gui/mod.rs"),
            // This test file references the literal pattern in its
            // own scanner code.
            Path::new("common/id_salt.rs"),
        ];

        let mut offenders = Vec::new();
        visit(&crate_root, &crate_root, whitelist, &mut offenders);

        assert!(
            offenders.is_empty(),
            "Found bare `UiBuilder` construction. Use `Gui::scoped_with` instead, \
             or annotate the line with `// id-drift-ok` if direct use is intentional. \
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
                    if !line.contains("UiBuilder::new(") {
                        continue;
                    }
                    // Allow whitelist on the same line OR up to two
                    // preceding non-blank lines (so we can put the
                    // justification comment above the call).
                    let same_line = line.contains("// id-drift-ok");
                    let preceding_ok = lines[..lineno]
                        .iter()
                        .rev()
                        .filter(|l| !l.trim().is_empty())
                        .take(2)
                        .any(|l| l.contains("// id-drift-ok"));
                    if !same_line && !preceding_ok {
                        offenders.push(format!("{}:{}: {}", path.display(), lineno + 1, trimmed));
                    }
                }
            }
        }
    }
}
