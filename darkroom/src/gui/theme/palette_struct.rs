//! The `palette_struct!` macro: one colour-roster declaration that also
//! mints its `DARK` / `LIGHT` presets, so a roster cannot gain a colour a
//! preset forgets.

/// Declares a colour-roster struct plus its two built-in instances
/// (`DARK` / `LIGHT`, pulling `dark::CONST` / `light::CONST`) from one
/// `field: Ty => CONST` list. One roster per struct, so a colour can't
/// sit in the struct while a preset forgets it: the presets won't
/// compile until every field is filled. The serialized
/// [`PaletteColors`](crate::gui::theme::palette_colors::PaletteColors) chrome roster is built this way; the
/// palantir-side rosters are plain [`palantir::Palette`] consts
/// (`PALANTIR_DARK` / `PALANTIR_LIGHT`).
///
/// Fields listed after a `;` are palette-independent — layout measurements,
/// mostly — given as `field: Ty = value` and copied verbatim into both
/// presets. That is what lets a per-widget group ([`CardTheme`](crate::gui::theme::card_theme::CardTheme),
/// [`PortTheme`](crate::gui::theme::port_theme::PortTheme), [`CanvasTheme`](crate::gui::theme::canvas_theme::CanvasTheme)) hold its geometry beside its colours
/// without the numbers being authored twice.
macro_rules! palette_struct {
    (
        $(#[$smeta:meta])*
        $vis:vis struct $name:ident;
        $($(#[$fmeta:meta])* $field:ident: $fty:ty => $konst:ident),+ $(,)?
        $(; $($(#[$dmeta:meta])* $dfield:ident: $dty:ty = $dval:expr),+ $(,)?)?
    ) => {
        $(#[$smeta])*
        $vis struct $name {
            $($(#[$fmeta])* $vis $field: $fty,)+
            $($($(#[$dmeta])* $vis $dfield: $dty,)+)?
        }

        impl $name {
            const DARK: Self = Self {
                $($field: dark::$konst,)+
                $($($dfield: $dval,)+)?
            };
            const LIGHT: Self = Self {
                $($field: light::$konst,)+
                $($($dfield: $dval,)+)?
            };

            /// This roster for `preset` — the per-group half of
            /// [`Theme::from_preset`](crate::gui::theme::Theme::from_preset).
            pub(super) fn for_preset(preset: ThemePreset) -> Self {
                match preset {
                    ThemePreset::Dark => Self::DARK,
                    ThemePreset::Light => Self::LIGHT,
                }
            }
        }
    };
}

pub(crate) use palette_struct;
