//! [`Theme`]: darkroom's visual palette and layout dimensions, plus the
//! per-widget rosters hanging off it.
//!
//! Every colour comes from [`Palette`], read out of the generated
//! `assets/ayu-graphite.ron`. Each per-widget roster owns a file and fills
//! itself from that palette through its own `from_palette`. The palantir-side
//! half lives in [`palantir_bridge`].

pub(crate) mod canvas_theme;
pub(crate) mod card_theme;
pub(crate) mod chrome_colors;
pub(crate) mod color;
pub(crate) mod const_value_editor_theme;
pub(crate) mod inline_rename_theme;
pub(crate) mod palantir_bridge;
pub(crate) mod palette;
pub(crate) mod port_theme;
pub(crate) mod status_colors;
pub(crate) mod type_colors;
pub(crate) mod type_scale;

use palantir::{ButtonTheme, FontWeight, RgbaF32, Stroke, TextEditTheme, TextStyle};

use crate::gui::theme::canvas_theme::CanvasTheme;
use crate::gui::theme::card_theme::CardTheme;
use crate::gui::theme::chrome_colors::ChromeColors;
use crate::gui::theme::const_value_editor_theme::ConstValueEditorTheme;
use crate::gui::theme::inline_rename_theme::InlineRenameTheme;
use crate::gui::theme::palantir_bridge::{
    BridgeRoles, menu_button_for, palantir_for, palantir_palette_for,
};
use crate::gui::theme::palette::Palette;
use crate::gui::theme::port_theme::PortTheme;
use crate::gui::theme::status_colors::StatusColors;
use crate::gui::theme::type_colors::TypeColors;
use crate::gui::theme::type_scale::TypeScale;

/// Visual palette + layout dimensions for darkroom's UI. Owned by `App`,
/// handed to every UI subtree through [`crate::gui::app::ctx::AppCtx`] and the
/// contexts derived from it, so call sites read off a single source
/// instead of hard-coded constants. Layout fields live here too —
/// node ports, value editors, etc. — so a theme swap can restyle
/// geometry as well as color.
///
/// Also owns the palantir [`palantir::Theme`] this app wants on its
/// `Ui`. [`crate::gui::app::App::new`] copies `palantir` into
/// `ui.theme` once before the first frame, so palantir-side widgets
/// (buttons, text edits, menus, scrollbars) read from the same source.
/// Tweak fields on `theme.palantir` during construction to
/// override palantir's defaults.
///
/// Built from the baked-in [`Palette`] on every launch; there is no theme
/// file.
#[derive(Clone, Debug)]
pub(crate) struct Theme {
    /// Stroke width of every mark drawn on the canvas at wire scale: the
    /// wires themselves, the in-flight drag preview, the subscription pin's
    /// leader, and the breaker scribble that cuts them — one width so the
    /// blade reads at the same weight as what it severs.
    pub(crate) stroke_width: f32,
    /// Gap between a node's edge and a floating widget's near edge — the
    /// inspector panel anchors from the node's right edge, so any future
    /// floating surface reads as the same clearance.
    pub(crate) floating_widget_gap: f32,
    /// Cap on the new-node popup's height. Inner scroll handles
    /// overflow when the function list exceeds the cap.
    pub(crate) new_node_popup_max_height: f32,

    /// Font sizes by hierarchy tier.
    pub(crate) text: TypeScale,

    /// The graph canvas and its dotted backdrop.
    pub(crate) canvas: CanvasTheme,

    /// Elevated rounded surfaces — node bodies, the inspector panel, dock
    /// tabs.
    pub(crate) card: CardTheme,

    /// A node's ports: circles, label ink, column geometry.
    pub(crate) ports: PortTheme,

    /// The semantic feedback palette — success / info / busy / warning /
    /// error.
    pub(crate) status: StatusColors,

    /// The colours belonging to no single widget.
    pub(crate) colors: ChromeColors,

    /// Data-type → wire/port hue roster (see [`TypeColors`]).
    pub(crate) type_colors: TypeColors,

    /// Look + dimensions for the inline static-value editor that hugs a
    /// `Binding::Const` input port (number/string field, file-pick chip).
    pub(crate) const_value_editor: ConstValueEditorTheme,

    /// The pointer-over-node variant of `const_value_editor` (chip fill
    /// pre-lit at half the hover strength). Precomputed at construction —
    /// deriving it per frame would clone the whole nested theme in the
    /// record path — and kept next to its base so the pair can't drift.
    pub(crate) const_value_editor_revealed: ConstValueEditorTheme,

    /// Look for the inline-rename widget. Text is left unset, so a rename
    /// inherits ambient `palantir::Theme::text` like any other label.
    pub(crate) inline_rename: InlineRenameTheme,

    /// The node-title variant of `inline_rename`, with the ambient text
    /// style pinned to [`FontWeight::BOLD`] on every state. Precomputed
    /// at construction, beside its base so the two can't drift — a node
    /// header would otherwise rebuild the whole nested text-edit bundle
    /// per node per frame just to carry one weight.
    pub(crate) inline_rename_title: InlineRenameTheme,

    /// The preferences path field's look while its committed path is broken:
    /// palantir's text edit with every state's border in the error colour.
    /// Precomputed at construction like `const_value_editor_revealed`, so a
    /// broken path costs no theme clone per frame.
    pub(crate) path_field_error: TextEditTheme,

    /// Look for a menu-bar trigger button. Darkroom's
    /// own slot: palantir ships the recipe but no theme field, because
    /// none of its widgets resolve against a menu-bar style — the bar
    /// passes this to [`palantir::Button::style`] itself.
    pub(crate) menu_button: ButtonTheme,

    /// Palantir-side widget theme. Pushed onto `Ui::theme` once at
    /// startup so every palantir widget (Button, `TextEdit`, `MenuItem`,
    /// Scroll, Tooltip…) reads a darkroom-tuned palette without each
    /// call site restyling per use.
    pub(crate) palantir: palantir::Theme,
}

impl Theme {
    /// How far a port circle of the given `radius` is pulled out of its
    /// column so its **center** lands on the node body's outer edge: clear
    /// the column inset (`port_col_pad_x`) and the body border
    /// (`node_border_width * 2`, which "folds into" the body's content
    /// padding), then push out by `radius` so the dot straddles the edge
    /// evenly. Parameterized rather than always `port_radius()` so an
    /// enlarged port (e.g. a required input's bigger circle) still
    /// straddles the edge correctly — see [`Self::port_overhang`] for the
    /// common (plain-radius) case.
    #[inline]
    pub(crate) fn port_overhang_for(&self, radius: f32) -> f32 {
        radius + self.ports.col_pad_x + self.card.border_width_total()
    }

    /// [`Self::port_overhang_for`] at the plain port radius. Independent of
    /// `port_size` — bigger circles keep their center on the edge.
    #[inline]
    pub(crate) fn port_overhang(&self) -> f32 {
        self.port_overhang_for(self.ports.radius())
    }

    /// Border color + width for a selectable card's 3-tier resting decision
    /// — how a node body resolves its outline: a breaker hit wins as the
    /// alarm color, else the selection halo
    /// when selected, else the neutral resting `node_border`. Width is
    /// always [`CardTheme::border_width_total`] regardless of tier, so
    /// selecting (or breaking) a card never resizes it — only the color
    /// changes. A
    /// caller with an extra tier of its own (e.g. a node body's "missing"
    /// stub state) special-cases that tier around this call instead of
    /// forcing it in here.
    #[inline]
    pub(crate) const fn card_border(&self, broken: bool, selected: bool) -> RgbaF32 {
        if broken {
            self.colors.connection_broken
        } else if selected {
            self.colors.selection_rect
        } else {
            self.card.border
        }
    }

    /// Assemble the full theme from `p` — the darkroom peer of
    /// `palantir::Theme::from_palette`. Dimensions are palette-independent;
    /// every colour and every sub-recipe (the palantir widget theme, the
    /// static-value editor, inline rename) cascades from `p` rather than
    /// being hand-assembled, so a palette edit reaches the whole app.
    fn build(p: &Palette) -> Self {
        let colors = ChromeColors::from_palette(p);
        let text = TypeScale::DEFAULT;
        // The palantir half is derived here rather than stored on `Palette`:
        // it is a projection of the same roles, and a second copy of them
        // could drift from the one the darkroom rosters read.
        let pal = palantir_palette_for(p);
        // Built before the struct literal because the title variant
        // derives from the palantir theme's ambient text style — the
        // same style an unstyled rename would have inherited anyway,
        // so bolding it is the only difference between the two slots.
        let card = CardTheme::from_palette(p);
        let status = StatusColors::from_palette(p);
        let palantir = palantir_for(
            &pal,
            BridgeRoles {
                chrome_fill: colors.chrome_fill,
                tab_inactive: colors.tab_inactive,
                header_fill: card.header_fill,
                warning: status.warning,
                corner_radius: card.corner_radius,
                text: &text,
            },
        );
        let menu_button = menu_button_for(&pal, palantir.text, &text);
        let path_field_error = error_bordered(&palantir.text_edit, status.error);
        let inline_rename = InlineRenameTheme::from_palette(&pal);
        let inline_rename_title = inline_rename.clone().with_text(TextStyle {
            weight: FontWeight::BOLD,
            ..palantir.text
        });
        Self {
            // The three measurements that belong to no widget group; the
            // rest are authored beside their colours in the groups below.
            stroke_width: 2.0,
            floating_widget_gap: 16.0,
            new_node_popup_max_height: 400.0,
            text,
            canvas: CanvasTheme::from_palette(p),
            card,
            ports: PortTheme::from_palette(p),
            status,
            colors,
            type_colors: p.type_colors.clone(),
            const_value_editor: ConstValueEditorTheme::from_palette(&pal),
            const_value_editor_revealed: ConstValueEditorTheme::revealed_from_palette(&pal),
            inline_rename,
            inline_rename_title,
            menu_button,
            path_field_error,
            palantir,
        }
    }
}

/// `base` with every state's border in `color`, its width kept.
fn error_bordered(base: &TextEditTheme, color: RgbaF32) -> TextEditTheme {
    let mut style = base.clone();
    for look in [
        &mut style.looks.normal,
        &mut style.looks.hovered,
        &mut style.looks.active,
    ] {
        look.background.border = Stroke::new(color, look.background.border.width);
    }
    style
}

impl Default for Theme {
    /// Ayu Graphite — the one built-in look, read from
    /// `assets/ayu-graphite.ron`.
    fn default() -> Self {
        Self::build(&Palette::load())
    }
}

#[cfg(test)]
mod tests;
