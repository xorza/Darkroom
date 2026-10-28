//! [`ConstEditorTheme`]: how an inline const editor is styled.

use palantir::{
    Brush, ButtonTheme, Chrome, DragValueTheme, FontFamily, RgbaF32, Spacing, TextEditTheme,
};

use crate::gui::theme::color;
use crate::gui::widgets::hover::Hover;

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

/// The const editor at rest and with the pointer over its node, built beside each other so they
/// cannot drift. Precomputed: deriving the revealed look per frame would clone the nested bundle
/// in the record path.
#[derive(Clone, Debug)]
pub(crate) struct ConstEditorLooks {
    pub(crate) rest: ConstEditorTheme,
    /// The boxes surface to [`REVEAL_ALPHA`] as soon as the pointer is anywhere over the node.
    /// Fill only, so geometry is identical to the resting look.
    pub(crate) revealed: ConstEditorTheme,
}

impl ConstEditorLooks {
    /// Shared shape: palantir's `menu_button` preset over `p` and `m` (no border) as the chip,
    /// inset as the bridged `field`, the app's text edit, with the inline editor derived from
    /// that chip so both modes share one box, and caret, selection and placeholder from `field`
    /// so it matches the app's other fields.
    pub(super) fn new(p: &palantir::Palette, m: &palantir::Metrics, field: &TextEditTheme) -> Self {
        let mut chip = ButtonTheme::menu_button(p, m);
        chip.defaults.padding = field.defaults.padding;
        let base = DragValueTheme::from_chip(chip, field);
        Self {
            rest: ConstEditorTheme::with_box(&base, p, REST_ALPHA),
            revealed: ConstEditorTheme::with_box(&base, p, REVEAL_ALPHA),
        }
    }

    /// The look for a node under the pointer or at rest.
    pub(crate) const fn at(&self, hover: Hover) -> &ConstEditorTheme {
        match hover {
            Hover::Over => &self.revealed,
            Hover::Rest => &self.rest,
        }
    }
}

impl ConstEditorTheme {
    /// The box a value sits in at rest, which the inspector draws a value in as the node does.
    pub(crate) const fn rest_box(&self) -> &Chrome {
        &self.drag_value.chip.looks.normal.chrome
    }

    /// The inset of the value in [`Self::rest_box`].
    pub(crate) const fn rest_padding(&self) -> Spacing {
        self.drag_value.chip.defaults.padding
    }

    /// `base` with the chip's and the editor's resting box at `alpha` of
    /// the hover fill. Both, so a numeric chip and a string field (always a
    /// `TextEdit`, showing `editor.normal` at rest) surface together.
    fn with_box(base: &DragValueTheme, p: &palantir::Palette, alpha: f32) -> Self {
        let mut drag_value = base.clone();
        let rest = Brush::Solid(color::wash(p.element_mid, alpha, p.element));
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
