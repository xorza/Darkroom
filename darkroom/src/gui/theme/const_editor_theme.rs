//! [`ConstEditorTheme`]: how an inline const editor is styled.

use palantir::{ButtonTheme, Chrome, DragValueTheme, FontFamily, RgbaF32, Spacing, TextEditTheme};

use crate::gui::theme::color;
use crate::gui::theme::hover::Hover;

const VALUE_EDITOR_WIDTH: f32 = 100.0;
/// Upper bound on the value column: a longer value ellipsizes instead of stretching the node.
const VALUE_EDITOR_MAX_WIDTH: f32 = 240.0;

/// Per-widget theme bundle for the inline const editor on a
/// `Binding::Const` input port: the `Button`/`ComboBox` look the path pick, enum and presets
/// wear, the `DragValue` look of a number in the same box, the text field a string types into,
/// and the fixed field width.
///
/// A value reads as a field at rest, as Blender's and Unreal's node inputs do: its chip wears a
/// quiet box, a step over the node's fill at half strength.
#[derive(Clone, Debug)]
pub(crate) struct ConstEditorTheme {
    /// The chip a path pick, an enum and a preset show.
    pub(crate) button: ButtonTheme,
    /// The text field a string, and a value of no declared kind, type into: [`Self::number`]'s
    /// editor in the proportional face.
    pub(crate) field: TextEditTheme,
    /// The chip a number scrubs on, and its editor, with the figures in the mono face, which
    /// then line up from row to row.
    pub(crate) number: DragValueTheme,
    /// The ink an empty input's "—" reads in: an optional one left unset, or one bound to
    /// nothing that could hold a constant.
    pub(crate) unset: RgbaF32,
    /// Minimum logical-px width of the value column.
    pub(crate) width: f32,
    /// Maximum logical-px width of the value column.
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
    /// Shared shape: palantir's `subtle` button over `p` and `m` (no border) as the chip,
    /// inset as the bridged `field`, the app's text edit, with the inline editor derived from
    /// that chip so both modes share one box, and caret, selection and placeholder from `field`
    /// so it matches the app's other fields.
    pub(super) fn new(p: &palantir::Palette, m: &palantir::Metrics, field: &TextEditTheme) -> Self {
        let mut button = ButtonTheme::subtle(p, m);
        button.defaults.padding = field.defaults.padding;
        Self {
            rest: ConstEditorTheme::with_box(&button, field, p, REST_ALPHA),
            revealed: ConstEditorTheme::with_box(&button, field, p, REVEAL_ALPHA),
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
        &self.button.unselected.normal.chrome
    }

    /// The inset of the value in [`Self::rest_box`].
    pub(crate) const fn rest_padding(&self) -> Spacing {
        self.button.defaults.padding
    }

    /// `button`'s resting box at `alpha` of the hover fill, worn by the button, the number and
    /// the field alike, so every kind of value surfaces together; the number and the field take
    /// the caret, selection and placeholder of the app's `field`.
    fn with_box(
        button: &ButtonTheme,
        field: &TextEditTheme,
        p: &palantir::Palette,
        alpha: f32,
    ) -> Self {
        let mut button = button.clone();
        button.unselected.normal.chrome.paint.fill =
            color::wash(p.element_mid, alpha, p.element).into();
        let proportional = DragValueTheme {
            looks: button.unselected.clone(),
            defaults: button.defaults,
            caret: field.caret,
            caret_width: field.caret_width,
            selection: field.selection,
            placeholder: field.placeholder,
        };
        let mut number = proportional.clone();
        for look in number.looks.each_mut() {
            look.text = look.text.with_family(FontFamily::MONO);
        }
        Self {
            button,
            field: proportional.editor(),
            number,
            unset: p.text_muted,
            width: VALUE_EDITOR_WIDTH,
            max_width: VALUE_EDITOR_MAX_WIDTH,
        }
    }
}
