//! [`ConstEditorTheme`]: how an inline const editor is styled.

use palantir::{Brush, ButtonTheme, DragValueTheme, FontFamily, RgbaF32, TextEditTheme};

const VALUE_EDITOR_WIDTH: f32 = 100.0;
/// Upper bound on the value column: editors fill the column up to here, then a
/// long value (a wide enum/preset dropdown, a long path) ellipsizes instead of
/// stretching the node out. Read by [`ConstEditorTheme::from_palette`],
/// which sizes the editor itself.
const VALUE_EDITOR_MAX_WIDTH: f32 = 240.0;

/// Per-widget theme bundle for the inline const editor on a
/// `Binding::Const` input port. Owns the `DragValue` look (scrub chip and
/// the inline editor derived from it) which the `Button`/`ComboBox`
/// siblings (path pick, enum, presets) borrow via `drag_value.chip`, the
/// numeric fields' mono variant of it, and the fixed field width.
///
/// A value reads as a field at rest, as Blender's and Unreal's node inputs
/// do: its chip wears a quiet box, a step over the node's fill at half
/// strength, so the eye finds what it can edit before the pointer arrives.
#[derive(Clone, Debug)]
pub(crate) struct ConstEditorTheme {
    pub(crate) drag_value: DragValueTheme,
    /// [`Self::drag_value`] with its text in the mono face, for a number's
    /// figures, which then line up from row to row.
    pub(crate) number: DragValueTheme,
    /// The ink an empty input's "—" reads in: an optional one left unset, or one bound to
    /// nothing that could hold a constant.
    pub(crate) unset: RgbaF32,
    /// Minimum logical-px width of the value column — editors fill it down to
    /// at least this.
    pub(crate) width: f32,
    /// Maximum logical-px width of the value column, so a wide editor (enum /
    /// preset dropdown, long path) ellipsizes rather than stretching the node.
    pub(crate) max_width: f32,
}

/// The resting box's strength, of the fill a hovered chip takes.
const REST_ALPHA: f32 = 0.5;
/// The box's strength while the pointer is over the node: the fields surface
/// together, short of the hovered chip's full fill.
const REVEAL_ALPHA: f32 = 0.8;

impl ConstEditorTheme {
    /// The pointer-over-node variant of [`Self::from_palette`]: the boxes
    /// surface to [`REVEAL_ALPHA`] as soon as the pointer is anywhere over
    /// the node. Fill only, so geometry is identical to the resting look.
    pub(super) fn revealed_from_palette(p: &palantir::Palette) -> Self {
        Self::with_box(p, REVEAL_ALPHA)
    }

    /// Shared shape: palantir's `menu_button` preset over `p` (no border) as
    /// the chip, with the inline editor derived from that chip so both modes
    /// share one box, and caret/selection/placeholder from the same palette's
    /// text-edit recipe so it matches the app's other text fields.
    pub(super) fn from_palette(p: &palantir::Palette) -> Self {
        Self::with_box(p, REST_ALPHA)
    }

    /// The bundle with the chip's and the editor's resting box at `alpha` of
    /// the hover fill. Both, so a numeric chip and a string field (always a
    /// `TextEdit`, showing `editor.normal` at rest) surface together.
    fn with_box(p: &palantir::Palette, alpha: f32) -> Self {
        let mut drag_value =
            DragValueTheme::from_chip(ButtonTheme::menu_button(p), &TextEditTheme::from_palette(p));
        let rest = Brush::Solid(p.element_mid.with_alpha(alpha));
        drag_value.chip.looks.normal.chrome.fill = rest.clone();
        drag_value.editor.looks.normal.chrome.fill = rest;
        let mut number = drag_value.clone();
        for look in number.chip.looks.each_mut() {
            look.text = look.text.with_family(FontFamily::MONO);
        }
        for look in number.editor.looks.each_mut() {
            look.text = look.text.with_family(FontFamily::MONO);
        }
        Self {
            drag_value,
            number,
            unset: p.text_muted,
            width: VALUE_EDITOR_WIDTH,
            max_width: VALUE_EDITOR_MAX_WIDTH,
        }
    }
}
