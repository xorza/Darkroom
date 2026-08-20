//! [`ConstValueEditorTheme`]: how an inline value editor is styled.

use palantir::{Brush, ButtonTheme, DragValueTheme, TextEditTheme};

// Layout dimensions are palette-independent — dark and light pull the same
// numbers. Each one's value lives on `Theme::build` (its field carries the doc
// comment); only the few read by more than one builder earn a name here. Font
// sizes are palette-independent too, and live on `TypeScale::DEFAULT`.
const VALUE_EDITOR_WIDTH: f32 = 100.0;
/// Upper bound on the value column: editors fill the column up to here, then a
/// long value (a wide enum/preset dropdown, a long path) ellipsizes instead of
/// stretching the node out. Read by both `Theme::build` and
/// [`ConstValueEditorTheme::from_palette`], which sizes the editor itself.
const VALUE_EDITOR_MAX_WIDTH: f32 = 240.0;

// The two preset swatch rosters live in `swatches.rs`, generated from
// `assets/ayu-graphite-base.toml` by `tools/build_palettes.py` alongside the
// two semantic palette TOMLs in `assets/`. Any builder (`Theme::dark`,
// `ConstValueEditorTheme::dark`, future per-widget helpers) reaches a swatch by
// name instead of inlining a hex literal, and the app reads the palette rather
// than a transcription of it: to restyle, edit the base and rerun the

/// Per-widget theme bundle for the inline static-value editor on a
/// `Binding::Const` input port. Owns the `DragValue` look (scrub chip —
/// transparent at rest, hover-only background, no border — plus the inline
/// editor derived from it) which the numeric fields use directly and the
/// `Button`/`ComboBox` siblings (path pick, enum, presets) borrow via
/// `drag_value.chip`, and the fixed field width.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ConstValueEditorTheme {
    pub(crate) drag_value: DragValueTheme,
    /// Minimum logical-px width of the value column — editors fill it down to
    /// at least this.
    pub(crate) width: f32,
    /// Maximum logical-px width of the value column, so a wide editor (enum /
    /// preset dropdown, long path) ellipsizes rather than stretching the node.
    pub(crate) max_width: f32,
}

impl ConstValueEditorTheme {
    /// The pointer-over-node variant of [`Self::from_palette`]: the
    /// chip's hover fill (`elem_hover`), at reduced alpha, becomes the
    /// *resting* background — const editors surface as soon as the
    /// pointer is anywhere over the node, without waiting for a direct
    /// hover. Fill only, so geometry is identical to the resting look.
    /// Built from the same palette recipe rather than patching a
    /// finished theme, so it can't drift from what the recipe painted.
    ///
    /// Both the resting `chip` (numeric editors, which show it at rest) and
    /// the inline `editor`'s normal state (string/`Any` editors, which are
    /// always a `TextEdit` and so show `editor.normal` at rest) get the same
    /// fill, so every field's edit affordance surfaces together.
    pub(super) fn revealed_from_palette(p: &palantir::Palette) -> Self {
        const REVEAL_ALPHA: f32 = 0.5;
        let mut out = Self::from_palette(p);
        let reveal = Brush::Solid(p.elem_hover.with_alpha(REVEAL_ALPHA));
        for bg in [
            &mut out.drag_value.chip.looks.normal.background,
            &mut out.drag_value.editor.looks.normal.background,
        ] {
            bg.fill = reveal.clone();
        }
        out
    }

    /// Shared shape: palantir's `menu_button` preset over `p` (transparent
    /// at rest + disabled, no border) as the chip, with the inline editor
    /// derived from that chip so both modes share one box, and
    /// caret/selection/placeholder from the same palette's text-edit
    /// recipe so it matches the app's other text fields.
    pub(super) fn from_palette(p: &palantir::Palette) -> Self {
        Self {
            drag_value: DragValueTheme::from_chip(
                ButtonTheme::menu_button(p),
                &TextEditTheme::from_palette(p),
            ),
            width: VALUE_EDITOR_WIDTH,
            max_width: VALUE_EDITOR_MAX_WIDTH,
        }
    }
}
