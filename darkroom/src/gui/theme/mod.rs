//! [`Theme`]: darkroom's visual palette and layout dimensions, plus the
//! per-widget rosters hanging off it.
//!
//! Every color comes from [`Palette`], read out of the generated
//! `assets/ayu-graphite.ron`, and every size from a scale: the type, space, radius and stroke
//! steps, which palantir's recipes read too, and darkroom's icon, elevation and tint scales. Each roster owns a file and builds from the palette and the scales it reads. The
//! palantir-side half lives in [`palantir_bridge`].

pub(crate) mod app_colors;
pub(crate) mod badge_colors;
pub(crate) mod canvas_theme;
pub(crate) mod card_theme;
pub(crate) mod color;
pub(crate) mod const_editor_theme;
pub(crate) mod elevation_scale;
pub(crate) mod flat_text_edit;
pub(crate) mod icon_scale;
pub(crate) mod inks;
pub(crate) mod palantir_bridge;
pub(crate) mod palette;
pub(crate) mod port_theme;
pub(crate) mod status_colors;
pub(crate) mod tie_theme;
pub(crate) mod tint_scale;
pub(crate) mod type_colors;
pub(crate) mod viewer_theme;

use palantir::{
    ButtonTheme, Chrome, Corners, Frost, Metrics, RadiusSteps, RgbaF32, SpaceSteps, Spacing,
    Stroke, StrokeSteps, TextEditTheme, TextSteps,
};

use scenarium::DataType;

use crate::gui::port_ref::PortKind;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::app_colors::AppColors;
use crate::gui::theme::badge_colors::BadgeColors;
use crate::gui::theme::canvas_theme::CanvasTheme;
use crate::gui::theme::card_theme::CardTheme;
use crate::gui::theme::const_editor_theme::ConstEditorLooks;
use crate::gui::theme::elevation_scale::ElevationScale;
use crate::gui::theme::flat_text_edit::FlatTextEdit;
use crate::gui::theme::icon_scale::IconScale;
use crate::gui::theme::inks::Inks;
use crate::gui::theme::palette::Palette;
use crate::gui::theme::port_theme::PortTheme;
use crate::gui::theme::status_colors::StatusColors;
use crate::gui::theme::tie_theme::TieTheme;
use crate::gui::theme::tint_scale::TintScale;
use crate::gui::theme::type_colors::TypeColors;
use crate::gui::theme::viewer_theme::ViewerTheme;

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
    /// Font sizes by hierarchy tier: [`TEXT`].
    pub(crate) font: TextSteps,

    /// The spacing steps every padding, gap and margin reads: [`SPACE`].
    pub(crate) space: SpaceSteps,

    /// Corner radii by the kind of surface: [`RADIUS`].
    pub(crate) radius: RadiusSteps,

    /// Line widths by what the line is: [`STROKE`].
    pub(crate) stroke: StrokeSteps,

    /// Icon boxes by where the icon sits.
    pub(crate) icon: IconScale,

    /// Shadows by how far a surface sits above the canvas.
    pub(crate) elevation: ElevationScale,

    /// The strengths a wash takes.
    pub(crate) tint: TintScale,

    /// The graph canvas and its dotted backdrop.
    pub(crate) canvas: CanvasTheme,

    /// A wire tie's band.
    pub(crate) tie: TieTheme,
    pub(crate) viewer: ViewerTheme,

    /// Elevated rounded surfaces — node bodies and inspector cards.
    pub(crate) card: CardTheme,

    /// A node's ports: circles, label ink, column geometry.
    pub(crate) port: PortTheme,

    /// The semantic feedback palette — success / info / busy / warning /
    /// error.
    pub(crate) status: StatusColors,

    /// The colors belonging to no single widget.
    pub(crate) colors: AppColors,

    /// The text inks.
    pub(crate) ink: Inks,

    /// What a node's head says about itself.
    pub(crate) badge: BadgeColors,

    /// Data-type → wire/port hue roster (see [`TypeColors`]).
    pub(crate) type_colors: TypeColors,

    /// The inline const editor that hugs a `Binding::Const` input port, at rest and with the
    /// pointer over its node.
    pub(crate) const_editor: ConstEditorLooks,

    /// A text edit that reads as plain text: a node title's rename, the palette's search field.
    pub(crate) flat_text_edit: FlatTextEdit,

    /// palantir's text edit with every state's border in the error color: a field whose value is
    /// broken, as a preferences path that names no file. Precomputed, so a broken field costs no
    /// theme clone per frame.
    pub(crate) field_error: TextEditTheme,

    /// A floating toolbar's pill: the surround's color over the popups' frosted glass, so a
    /// readout on it holds its contrast over the image under it.
    pub(crate) pill: Chrome,

    /// A dialog's primary action, the one Return picks: palantir's button filled with the accent.
    /// Darkroom's own slot, as no palantir widget resolves against it.
    pub(crate) primary_button: ButtonTheme,

    /// Palantir-side widget theme. Pushed onto `Ui::theme` once at
    /// startup so every palantir widget (Button, `TextEdit`, `MenuItem`,
    /// Scroll, Tooltip…) reads a darkroom-tuned palette without each
    /// call site restyling per use.
    pub(crate) palantir: palantir::Theme,
}

/// [`Theme::row_height`] in body sizes.
const ROW_HEIGHT_EM: f32 = 2.0;

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
            DataType::Any => self.port.input,
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

    /// The ink of a node's last-run `status` in its footer and its inspector: the muted ink for a
    /// node that did not run, and for one a failure before it stopped.
    pub(crate) const fn exec_ink(&self, status: &ExecStatus) -> RgbaF32 {
        match status {
            ExecStatus::None | ExecStatus::Skipped => self.ink.muted,
            ExecStatus::Cached => self.status.info,
            ExecStatus::Executed(_) => self.status.success,
            ExecStatus::Running(_) => self.status.busy,
            ExecStatus::MissingInputs(_) => self.status.warning,
            ExecStatus::Errored(_) => self.status.error,
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

    /// How far a port circle of `radius` is pulled out of its column so its center lands on the
    /// node body's outer edge: past the column inset (`port.col_pad_x`) and the body's stroke
    /// ([`CardTheme::border_width`], which folds into the body's content padding), then out
    /// by `radius`, so the dot straddles the edge evenly. An enlarged port, a required input's
    /// bigger circle, passes its own radius.
    #[inline]
    fn port_overhang_for(&self, radius: f32) -> f32 {
        radius + self.port.col_pad_x + self.card.border_width
    }

    /// The margin that hangs a port glyph of `radius` on the body's edge on `kind`'s side, its
    /// center on the edge, and takes back the glyph's width and its cell's `space.xs` gap, so the
    /// label beside it starts at the column inset whatever the glyph's size, and where a row has
    /// no glyph: every label then lines up with the header's and footer's text.
    pub(crate) fn port_margin(&self, kind: PortKind, radius: f32) -> Spacing {
        let overhang = self.port_overhang_for(radius);
        let back = overhang - 2.0 * radius - self.space.xs;
        match kind {
            PortKind::Input => Spacing::new(-overhang, 0.0, back, 0.0),
            PortKind::Output => Spacing::new(back, 0.0, -overhang, 0.0),
        }
    }

    /// A card's chrome at rest: its fill, the card radius and the resting shadow. A node adds its
    /// border and trades the shadow for a status frame.
    pub(crate) fn card_chrome(&self) -> Chrome {
        Chrome::rounded(self.card.fill, Corners::all(self.radius.card))
            .with_shadow(self.elevation.resting)
    }

    /// The corner radius of a strip seated flush inside a card's stroke, a footer: the card's
    /// radius less the stroke, else the strip's corner leaves a wedge of fill showing between it
    /// and a selection-lit stroke.
    #[inline]
    pub(crate) fn card_inner_radius(&self) -> f32 {
        (self.radius.card - self.card.border_width).max(0.0)
    }

    /// The fill of a control that is on, an icon button's or a toolbar chip's: a wash of its
    /// `ink` over the card's fill, a step stronger under the pointer.
    pub(crate) fn on_fill(&self, ink: RgbaF32, hovered: bool) -> RgbaF32 {
        let strength = if hovered {
            self.tint.strong
        } else {
            self.tint.soft
        };
        color::wash(ink, strength, self.card.fill)
    }

    /// The inset of a pane's content from its edges, every pane's: the inspector's and the
    /// preferences'.
    #[inline]
    pub(crate) fn pane_padding(&self) -> Spacing {
        Spacing::all(self.space.lg)
    }

    /// Where the surround's controls start from the window's edge: the menu bar's first title and
    /// the first tab, whose text then sits a control's `space.md` further in, as the status bar's
    /// does. palantir's tab strip and menu bar pad their bands by the same `space.sm`.
    #[inline]
    pub(crate) const fn surround_inset(&self) -> f32 {
        self.space.sm
    }

    /// The height of a row that holds a field: a node's port row, an inspector's port row. Two
    /// body lines, which clears the tallest field, the inline text edit, at a line and its
    /// padding, about 1.9 body sizes.
    #[inline]
    pub(crate) fn row_height(&self) -> f32 {
        self.font.body * ROW_HEIGHT_EM
    }

    /// Border color for a selectable card in `tier`. Its width is
    /// [`CardTheme::border_width`] in every tier, so selecting (or
    /// breaking) a card never resizes it — only the color changes. A caller
    /// with an extra tier of its own (e.g. a node body's "missing" stub state)
    /// special-cases that tier around this call instead of forcing it in here.
    #[inline]
    pub(crate) const fn card_border(&self, tier: CardTier) -> RgbaF32 {
        match tier {
            CardTier::Broken => self.canvas.wire_broken,
            CardTier::Selected => self.colors.accent,
            CardTier::Resting => self.card.border,
        }
    }

    /// Assemble the full theme from `p` — the darkroom peer of
    /// `palantir::Theme::from_parts`. Dimensions are palette-independent;
    /// every color and every sub-recipe (the palantir widget theme, the
    /// const editor, inline rename) cascades from `p` rather than
    /// being hand-assembled, so a palette edit reaches the whole app.
    fn build(p: &Palette) -> Self {
        let colors = AppColors::from_palette(p);
        let (font, space, radius, stroke) = (TEXT, SPACE, RADIUS, STROKE);
        let icon = IconScale::new(&font);
        let elevation = ElevationScale::new(p);
        let tint = TintScale::DEFAULT;
        let ink = Inks::from_palette(p);
        // The palantir half is derived here rather than stored on `Palette`:
        // it is a projection of the same roles, and a second copy of them
        // could drift from the one the darkroom rosters read.
        let pal = palantir_bridge::palette(p);
        let card = CardTheme::from_palette(p);
        let status = StatusColors::from_palette(p);
        let metrics = metrics(&elevation);
        let palantir = palantir_bridge::theme(&pal, &metrics, &icon);
        let primary_button = palantir_bridge::primary_button(&pal, &metrics);
        let pill = Chrome::rounded(colors.surround_bg, Corners::all(radius.float))
            .with_frost(frost(ink.muted));
        let field_error = error_bordered(&palantir.text_edit, status.error);
        Self {
            canvas: CanvasTheme::from_palette(p, &space),
            tie: TieTheme::from_palette(p, &tint),
            viewer: ViewerTheme::new(p, &space, &ink),
            card,
            port: PortTheme::from_palette(p, &space),
            font,
            space,
            radius,
            stroke,
            icon,
            elevation,
            tint,
            status,
            colors,
            ink,
            badge: BadgeColors::from_palette(p),
            type_colors: p.type_colors.clone(),
            const_editor: ConstEditorLooks::new(&pal, &metrics, &palantir.text_edit),
            flat_text_edit: FlatTextEdit::new(&palantir.text_edit),
            field_error,
            pill,
            primary_button,
            palantir,
        }
    }
}

/// Font sizes by prominence, the Windows Fluent 2 ramp (Caption 12, Body 14, Subtitle 16): two
/// points between tiers, a step the eye resolves. A literal at a call site is a missing tier.
/// `darkroom/AGENTS.md`, "The look", "One type ladder for darkroom and palantir".
const TEXT: TextSteps = TextSteps {
    caption: 12.0,
    body: 14.0,
    title: 16.0,
};

/// A 4 px grid, in steps of 4 to 16 and of 8 past it, with a 2 px step under it for the insets of
/// the densest rows. A literal at a call site is a missing step.
const SPACE: SpaceSteps = SpaceSteps {
    xxs: 2.0,
    xs: 4.0,
    sm: 8.0,
    md: 12.0,
    lg: 16.0,
    xl: 24.0,
};

/// How round a surface is, by what it is. A toolbar pill is concentric with the card-rounded chips
/// inside it at the `xs` padding.
const RADIUS: RadiusSteps = RadiusSteps {
    mark: 2.0,
    control: 4.0,
    card: 6.0,
    float: 10.0,
};

/// The widths every stroke reads, by what the stroke is; a wire's is the canvas's
/// ([`CanvasTheme::wire`]).
const STROKE: StrokeSteps = StrokeSteps {
    hairline: 1.0,
    fine: 1.5,
    line: 2.0,
};

/// The steps palantir's recipes read, so a palantir control lines up with a darkroom one: a
/// field and a button beside it share one height, a menu rounds and casts as every floating
/// surface does. No transition: darkroom's controls snap, so a view saved right after a menu
/// pick pictures the settled window. `darkroom/AGENTS.md`, "The look", "palantir's recipes read
/// the theme's steps".
const fn metrics(elevation: &ElevationScale) -> Metrics {
    Metrics {
        text: TEXT,
        space: SPACE,
        radius: RADIUS,
        stroke: STROKE,
        float_shadow: elevation.floating,
        transition: None,
    }
}

/// The frosted glass every surface that floats over content wears, the popups and the toolbar
/// pills: its own color as a tint at 0.72 alpha over a backdrop blurred by σ 20 logical px, wide
/// enough that a node's title under it reads as color, not as text. `ink` is the dimmest ink the
/// surface paints. `darkroom/AGENTS.md`, "The look", "A surface that floats over content is
/// frosted glass".
pub(super) const fn frost(ink: RgbaF32) -> Frost {
    Frost {
        blur: 20.0,
        tint_alpha: 0.72,
        ink,
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
