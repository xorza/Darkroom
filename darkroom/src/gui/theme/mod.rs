//! [`Theme`]: darkroom's visual palette and layout dimensions, plus the
//! per-widget rosters hanging off it.
//!
//! Every color comes from [`Palette`], read out of the generated
//! `assets/ayu-graphite.ron`. Each per-widget roster owns a file and fills
//! itself from that palette through its own `from_palette`. The palantir-side
//! half lives in [`palantir_bridge`].

pub(crate) mod app_colors;
pub(crate) mod canvas_theme;
pub(crate) mod card_theme;
pub(crate) mod color;
pub(crate) mod const_editor_theme;
pub(crate) mod inline_rename_theme;
pub(crate) mod palantir_bridge;
pub(crate) mod palette;
pub(crate) mod port_theme;
pub(crate) mod radius_scale;
pub(crate) mod space_scale;
pub(crate) mod status_colors;
pub(crate) mod tie_theme;
pub(crate) mod type_colors;
pub(crate) mod type_scale;

use palantir::{ButtonTheme, FontWeight, RgbaF32, Stroke, TextEditTheme, TextStyleOverrides};

use scenarium::DataType;

use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::app_colors::AppColors;
use crate::gui::theme::canvas_theme::CanvasTheme;
use crate::gui::theme::card_theme::CardTheme;
use crate::gui::theme::const_editor_theme::ConstEditorTheme;
use crate::gui::theme::inline_rename_theme::InlineRenameTheme;
use crate::gui::theme::palantir_bridge::BridgeRoles;
use crate::gui::theme::palette::Palette;
use crate::gui::theme::port_theme::PortTheme;
use crate::gui::theme::radius_scale::RadiusScale;
use crate::gui::theme::space_scale::SpaceScale;
use crate::gui::theme::status_colors::StatusColors;
use crate::gui::theme::tie_theme::TieTheme;
use crate::gui::theme::type_colors::TypeColors;
use crate::gui::theme::type_scale::TypeScale;

/// Visual palette + layout dimensions for darkroom's UI. Owned by `App`,
/// handed to every UI subtree through [`crate::gui::frame::app_ctx::AppCtx`] and the
/// contexts derived from it, so call sites read off a single source
/// instead of hard-coded constants. Layout fields live here too —
/// node ports, const editors, etc. — so a theme swap can restyle
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
    /// Cap on the new-node popup's height. Inner scroll handles
    /// overflow when the function list exceeds the cap.
    pub(crate) new_node_popup_max_height: f32,

    /// Font sizes by hierarchy tier.
    pub(crate) text: TypeScale,

    /// The spacing steps every padding, gap and margin reads.
    pub(crate) space: SpaceScale,

    /// Corner radii by the kind of surface.
    pub(crate) radius: RadiusScale,

    /// The graph canvas and its dotted backdrop.
    pub(crate) canvas: CanvasTheme,

    /// A wire tie's band.
    pub(crate) tie: TieTheme,

    /// Elevated rounded surfaces — node bodies and dock tabs.
    pub(crate) card: CardTheme,

    /// A node's ports: circles, label ink, column geometry.
    pub(crate) ports: PortTheme,

    /// The semantic feedback palette — success / info / busy / warning /
    /// error.
    pub(crate) status: StatusColors,

    /// The colors belonging to no single widget.
    pub(crate) colors: AppColors,

    /// Data-type → wire/port hue roster (see [`TypeColors`]).
    pub(crate) type_colors: TypeColors,

    /// Look + dimensions for the inline const editor that hugs a
    /// `Binding::Const` input port (number/string field, file-pick chip).
    pub(crate) const_editor: ConstEditorTheme,

    /// The pointer-over-node variant of `const_editor` (chip fill
    /// pre-lit at half the hover strength). Precomputed at construction —
    /// deriving it per frame would clone the whole nested theme in the
    /// record path — and kept next to its base so the pair can't drift.
    pub(crate) const_editor_revealed: ConstEditorTheme,

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
    /// palantir's text edit with every state's border in the error color.
    /// Precomputed at construction like `const_editor_revealed`, so a
    /// broken path costs no theme clone per frame.
    pub(crate) path_field_error: TextEditTheme,

    /// Look for a menu-bar trigger button. Darkroom's own slot: palantir
    /// ships the recipe but no theme field, because none of its widgets
    /// resolve against a menu-bar style — the bar passes this to
    /// [`palantir::Button::style`] itself.
    pub(crate) menu_button: ButtonTheme,
    /// [`Self::menu_button`] while its menu is open: lit at rest as when hovered, as macOS,
    /// Windows and GNOME light the title of the open menu.
    pub(crate) menu_button_open: ButtonTheme,

    /// Palantir-side widget theme. Pushed onto `Ui::theme` once at
    /// startup so every palantir widget (Button, `TextEdit`, `MenuItem`,
    /// Scroll, Tooltip…) reads a darkroom-tuned palette without each
    /// call site restyling per use.
    pub(crate) palantir: palantir::Theme,
}

/// How a selectable card's outline resolves, highest first: a breaker hit
/// reads as the alarm, else the selection halo, else the neutral resting
/// border.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardTier {
    Broken,
    Selected,
    Resting,
}

impl Theme {
    /// The color a port of `data_type` paints at rest, and the wires touching it; see
    /// [`Hover::lift`](crate::gui::widgets::hover::Hover::lift). The built-in scalars and the
    /// lens image type, the dominant payload on a darkroom canvas, take a fixed hue each from
    /// [`TypeColors`]; another custom or enum type lands on a ramp slot by its type id, so
    /// distinct types read distinct and stay stable. `Any`, which only an input declares, has no
    /// type identity and takes the untyped input color.
    pub(crate) fn port_color(&self, data_type: &DataType) -> RgbaF32 {
        let hues = &self.type_colors;
        match data_type {
            DataType::Any => self.ports.input,
            DataType::Bool => hues.boolean,
            DataType::Int => hues.int,
            DataType::Float => hues.float,
            DataType::String => hues.string,
            DataType::FsPath(_) => hues.path,
            DataType::Custom(id) if *id == lens::IMAGE_TYPE_ID => hues.image,
            DataType::Custom(id) | DataType::Enum(id) => {
                let slot = id.as_u128() % hues.ramp.len() as u128;
                hues.ramp[usize::try_from(slot).expect("a slot is below the ramp's length")]
            }
        }
    }

    /// The color of an event emitter glyph, a subscription pin and an event wire, at rest:
    /// events carry no data type, so one color serves them all.
    pub(crate) const fn event_color(&self) -> RgbaF32 {
        self.ports.event
    }

    /// The ink of a node's last-run `status` in its footer and its inspector, or `None` for the
    /// muted ink: a node that did not run, and one a failure before it stopped.
    pub(crate) const fn exec_color(&self, status: &ExecStatus) -> Option<RgbaF32> {
        match status {
            ExecStatus::None | ExecStatus::Skipped => None,
            ExecStatus::Cached => Some(self.status.info),
            ExecStatus::Executed(_) => Some(self.status.success),
            ExecStatus::Running(_) => Some(self.status.busy),
            ExecStatus::MissingInputs(_) => Some(self.status.warning),
            ExecStatus::Errored(_) => Some(self.status.error),
        }
    }

    /// The frame a node's body takes for its last-run `status`, or `None` for its resting
    /// shadow. Only what asks for attention takes one — a node in progress, one the run could
    /// not feed, and the node that failed — so a good run leaves the canvas quiet, and a
    /// failure frames its cause alone, not each node it stopped.
    pub(crate) const fn exec_frame(&self, status: &ExecStatus) -> Option<RgbaF32> {
        match status {
            ExecStatus::None
            | ExecStatus::Cached
            | ExecStatus::Executed(_)
            | ExecStatus::Skipped => None,
            ExecStatus::Running(_) => Some(self.status.busy),
            ExecStatus::MissingInputs(_) => Some(self.status.warning),
            ExecStatus::Errored(_) => Some(self.status.error),
        }
    }

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

    /// Border color for a selectable card in `tier`. Width is always
    /// [`CardTheme::border_width_total`] regardless of tier, so selecting (or
    /// breaking) a card never resizes it — only the color changes. A caller
    /// with an extra tier of its own (e.g. a node body's "missing" stub state)
    /// special-cases that tier around this call instead of forcing it in here.
    #[inline]
    pub(crate) const fn card_border(&self, tier: CardTier) -> RgbaF32 {
        match tier {
            CardTier::Broken => self.colors.wire_broken,
            CardTier::Selected => self.colors.selection_rect,
            CardTier::Resting => self.card.border,
        }
    }

    /// Assemble the full theme from `p` — the darkroom peer of
    /// `palantir::Theme::from_palette`. Dimensions are palette-independent;
    /// every color and every sub-recipe (the palantir widget theme, the
    /// const editor, inline rename) cascades from `p` rather than
    /// being hand-assembled, so a palette edit reaches the whole app.
    fn build(p: &Palette) -> Self {
        let colors = AppColors::from_palette(p);
        let text = TypeScale::DEFAULT;
        let space = SpaceScale::DEFAULT;
        let radius = RadiusScale::DEFAULT;
        // The palantir half is derived here rather than stored on `Palette`:
        // it is a projection of the same roles, and a second copy of them
        // could drift from the one the darkroom rosters read.
        let pal = palantir_bridge::palette(p);
        let card = CardTheme::from_palette(p, &radius);
        let status = StatusColors::from_palette(p);
        let palantir = palantir_bridge::theme(
            &pal,
            BridgeRoles {
                surround_fill: colors.surround_fill,
                tab_inactive: colors.tab_inactive,
                header_fill: card.header_fill,
                warning: status.warning,
                text: &text,
                space: &space,
                radius: &radius,
            },
        );
        let menu_button = palantir_bridge::menu_button(&pal);
        let menu_button_open = palantir_bridge::menu_button_open(&menu_button);
        let path_field_error = error_bordered(&palantir.text_edit, status.error);
        let inline_rename = InlineRenameTheme::from_palette(&pal);
        let inline_rename_title = inline_rename
            .clone()
            .with_text(TextStyleOverrides::NONE.with_weight(FontWeight::BOLD));
        Self {
            // The two measurements that belong to no widget group; the
            // rest are authored beside their colors in the groups below.
            stroke_width: 2.0,
            new_node_popup_max_height: 400.0,
            text,
            space,
            radius,
            canvas: CanvasTheme::from_palette(p),
            tie: TieTheme::from_palette(p),
            card,
            ports: PortTheme::from_palette(p),
            status,
            colors,
            type_colors: p.type_colors.clone(),
            const_editor: ConstEditorTheme::from_palette(&pal),
            const_editor_revealed: ConstEditorTheme::revealed_from_palette(&pal),
            inline_rename,
            inline_rename_title,
            menu_button,
            menu_button_open,
            path_field_error,
            palantir,
        }
    }
}

/// `base` with every state's border in `color`, its width kept.
fn error_bordered(base: &TextEditTheme, color: RgbaF32) -> TextEditTheme {
    let mut style = base.clone();
    for look in style.looks.each_mut() {
        look.chrome.border = Stroke::new(color, look.chrome.border.width);
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
