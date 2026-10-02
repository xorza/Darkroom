//! Which per-frame processing configs the editor can build, and the projections
//! for the ones whose shape it cannot edit directly.
//!
//! Most of these are simply the lumos config: it derives
//! [`Introspect`](common::Introspect) itself, so the builder node's ports *are*
//! its fields and adding one in lumos adds a port here with no edit on this
//! side. Their wire identity — a `TYPE_ID` that ships in saved documents and so
//! is fixed for the life of the type, and a `DISPLAY_NAME` that is only what the
//! editor labels that wire — is declared by their own derive in lumos.
//!
//! A config the field model can't express — one whose enum variants carry
//! data, which [`IntrospectEnum`](common::IntrospectEnum) does not describe —
//! gets a projection instead: a flat struct of the knobs the editor offers,
//! plus the one-way conversion that expands them back into the real config. A
//! projection is deliberately narrower than the type it builds, so it does
//! *not* track that type field-for-field.

use common::{Introspect, IntrospectEnum};
use lumos::{BackgroundMode, ColorMode, ExtractBackground, Scnr, Stretch, StretchMethod};

use crate::astro::config::preset::preset_enum;

const SCNR_ADDITIVE_AMOUNT: f32 = 0.5;

preset_enum! {
    StretchPreset => Stretch,
    display: "StretchPreset",
    variants: {
        AutoAsinh = "auto_asinh" @ "Auto Asinh" => Stretch::auto_asinh(),
        AutoStf = "auto_stf" @ "Auto STF" => Stretch::auto_stf(),
    }
}

preset_enum! {
    BackgroundModeKind => ExtractBackground,
    display: "BackgroundMode",
    variants: {
        Subtract = "subtract" @ "Subtract" => ExtractBackground {
            mode: BackgroundMode::Subtract,
            ..Default::default()
        },
        Divide = "divide" @ "Divide" => ExtractBackground {
            mode: BackgroundMode::Divide,
            ..Default::default()
        },
    }
}

preset_enum! {
    ScnrKind => Scnr,
    display: "Scnr",
    variants: {
        AverageNeutral = "average_neutral" @ "Average Neutral" => Scnr::average_neutral(),
        AdditiveMask = "additive_mask" @ "Additive Mask" => Scnr::additive_mask(SCNR_ADDITIVE_AMOUNT),
    }
}

/// Which green-removal protection [`ScnrKnobs`] builds. The lumos enum carries
/// the additive mask's blend amount in its variant, so the editor picks the
/// method here and supplies the amount as its own field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "662e2432-b685-4b5b-bf05-0041814dc908")]
pub(crate) enum ScnrMethodChoice {
    AverageNeutral,
    AdditiveMask,
}

/// The editable knobs behind a [`Scnr`]. `amount` is read only by
/// [`ScnrMethodChoice::AdditiveMask`]; average-neutral is a full-strength clamp
/// with nothing to tune.
#[derive(Debug, Clone, Introspect)]
#[config(type_id = "cb80e688-a5ed-42fd-9087-6a9639a8b056", name = "ScnrConfig")]
pub(crate) struct ScnrKnobs {
    method: ScnrMethodChoice,
    amount: f32,
}

impl Default for ScnrKnobs {
    fn default() -> Self {
        Self {
            method: ScnrMethodChoice::AverageNeutral,
            amount: SCNR_ADDITIVE_AMOUNT,
        }
    }
}

impl From<ScnrKnobs> for Scnr {
    fn from(knobs: ScnrKnobs) -> Self {
        match knobs.method {
            ScnrMethodChoice::AverageNeutral => Scnr::average_neutral(),
            ScnrMethodChoice::AdditiveMask => Scnr::additive_mask(knobs.amount),
        }
    }
}

/// Which stretch curve [`StretchKnobs`] builds — the two automatic methods.
/// [`StretchMethod`]'s explicit curves (`Asinh`, `Ghs`) are not offered: each
/// carries its own parameter set, which one flat knob list cannot present
/// without showing every other method's parameters alongside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "722f7047-a6fc-4538-abd7-8af5fd1ee0ff")]
pub(crate) enum StretchMethodChoice {
    AutoAsinh,
    AutoStf,
}

/// The editable knobs behind a [`Stretch`]. Both methods take a
/// `target_background`; `shadow_sigmas` is read only by
/// [`StretchMethodChoice::AutoStf`].
#[derive(Debug, Clone, Introspect)]
#[config(type_id = "b08bb9a1-db12-43d4-aa57-fe3e3732e917", name = "Stretch")]
pub(crate) struct StretchKnobs {
    method: StretchMethodChoice,
    target_background: f32,
    shadow_sigmas: f32,
    color: ColorMode,
}

impl Default for StretchKnobs {
    fn default() -> Self {
        let config = Stretch::default();
        let StretchMethod::AutoAsinh { target_background } = config.method else {
            panic!("lumos Stretch::default() must remain auto-asinh");
        };
        Self {
            method: StretchMethodChoice::AutoAsinh,
            target_background,
            shadow_sigmas: 1.5,
            color: config.color,
        }
    }
}

impl From<StretchKnobs> for Stretch {
    fn from(knobs: StretchKnobs) -> Self {
        let method = match knobs.method {
            StretchMethodChoice::AutoAsinh => StretchMethod::AutoAsinh {
                target_background: knobs.target_background,
            },
            StretchMethodChoice::AutoStf => StretchMethod::AutoStf {
                shadow_sigmas: knobs.shadow_sigmas,
                target_background: knobs.target_background,
            },
        };
        Stretch {
            method,
            color: knobs.color,
        }
    }
}

#[cfg(test)]
mod tests {
    use common::{Introspect, IntrospectEnum};
    use lumos::{
        BackgroundMode, ColorMode, Denoise, ExtractBackground, Hdr, LocalContrast, Stretch,
        StretchMethod, Threshold,
    };

    use crate::astro::config::processing::{StretchKnobs, StretchMethodChoice};

    fn field_names<T: Introspect>() -> Vec<&'static str> {
        T::fields().into_iter().map(|field| field.name).collect()
    }

    /// A builder node's ports are its config's fields, in declaration order,
    /// and a saved graph binds them by position — so reordering or renaming a
    /// field in lumos silently rewires every document that used the node.
    /// Pinned here because the config types live in another crate now: this is
    /// what makes the coupling visible from the side that depends on it.
    #[test]
    fn builder_ports_follow_the_lumos_field_order() {
        assert_eq!(
            field_names::<ExtractBackground>(),
            [
                "tile_size",
                "degree",
                "mode",
                "rejection_sigma",
                "iterations",
                "divide_floor"
            ]
        );
        assert_eq!(
            field_names::<Denoise>(),
            ["scales", "k", "threshold", "strength"]
        );
        assert_eq!(field_names::<Hdr>(), ["scales", "amount"]);
        assert_eq!(
            field_names::<LocalContrast>(),
            ["tiles", "clip_limit", "strength"]
        );
        assert_eq!(
            field_names::<StretchKnobs>(),
            ["method", "target_background", "shadow_sigmas", "color"]
        );
    }

    /// The variant strings are what a saved graph stores for an enum port, and
    /// the derive renders them from the variant names — so a rename in lumos
    /// would change what is already on disk.
    #[test]
    fn enum_ports_keep_their_stored_variant_names() {
        assert_eq!(BackgroundMode::VARIANTS, ["subtract", "divide"]);
        assert_eq!(Threshold::VARIANTS, ["hard", "soft"]);
        assert_eq!(ColorMode::VARIANTS, ["color_preserving", "per_channel"]);
        assert_eq!(StretchMethodChoice::VARIANTS, ["auto_asinh", "auto_stf"]);
    }

    #[test]
    fn stretch_default_and_supported_methods_convert_exactly() {
        let default = StretchKnobs::default();
        assert_eq!(default.method, StretchMethodChoice::AutoAsinh);
        assert_eq!(default.color, ColorMode::ColorPreserving);

        let stretch: Stretch = StretchKnobs {
            method: StretchMethodChoice::AutoStf,
            target_background: 0.25,
            shadow_sigmas: 2.0,
            color: ColorMode::PerChannel,
        }
        .into();
        let StretchMethod::AutoStf {
            shadow_sigmas,
            target_background,
        } = stretch.method
        else {
            panic!("expected auto-STF");
        };
        assert_eq!(shadow_sigmas, 2.0);
        assert_eq!(target_background, 0.25);
        assert_eq!(stretch.color, ColorMode::PerChannel);
    }
}
