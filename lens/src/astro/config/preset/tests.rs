use std::fmt;

use lumos::{
    BgMode, ColorMode, Combine, ExtractBg, RegistrationConfig, Scnr, ScnrProtection, StackConfig,
    StarDetectionConfig, Stretch, StretchMethod,
};
use scenarium::{ConstValue, DynamicValue};

use crate::astro::config::preset::Preset;
use crate::astro::config::processing::{StretchKnobs, StretchMethodChoice};
use crate::astro::config::stacking::{CombineMethodChoice, DetectionPreset, RegistrationPreset};
use crate::config_node::{ConfigValue, config_data_type};

/// The stored strings and the labels the input offers, pinned once per
/// family: a stored string is what a saved graph holds, so a rename is a
/// format change. The input is required and wireable, of the detailed
/// config's wire type, and starts on the first pick.
#[test]
fn each_input_offers_its_variants_under_their_labels() {
    fn offered<P: Preset>() -> Vec<(String, &'static str)> {
        let input = P::input("Pick");
        assert!(input.required && !input.const_only);
        assert_eq!(input.data_type, config_data_type::<P::Knobs>());
        assert_eq!(
            input.default_value,
            Some(ConstValue::Enum(P::VARIANTS[0].to_owned()))
        );
        input
            .value_variants
            .iter()
            .map(|variant| {
                let label = P::LABELS[P::VARIANTS.iter().position(|v| *v == variant.name).unwrap()];
                assert_eq!(variant.label(), label);
                (variant.name.clone(), label)
            })
            .collect()
    }
    let names = |offered: Vec<(String, &'static str)>| -> Vec<String> {
        offered.into_iter().map(|(name, _)| name).collect()
    };

    assert_eq!(
        offered::<StretchMethodChoice>(),
        [
            ("auto_asinh".to_owned(), "Auto Asinh"),
            ("auto_stf".to_owned(), "Auto STF")
        ]
    );
    assert_eq!(names(offered::<BgMode>()), ["subtract", "divide"]);
    assert_eq!(
        names(offered::<ScnrProtection>()),
        [
            "average_neutral",
            "additive_mask",
            "maximum_neutral",
            "maximum_mask"
        ]
    );
    assert_eq!(
        names(offered::<DetectionPreset>()),
        [
            "wide_field",
            "high_resolution",
            "crowded_field",
            "precise_ground"
        ]
    );
    assert_eq!(
        names(offered::<RegistrationPreset>()),
        ["default", "fast", "precise", "wide_field", "mosaic"]
    );
    assert_eq!(
        names(offered::<CombineMethodChoice>()),
        ["sigma_clipped", "winsorized", "median", "mean"]
    );
}

/// Each pick builds exactly the lumos preset it names.
#[test]
fn each_pick_builds_the_preset_it_names() {
    let same = |a: &dyn fmt::Debug, b: &dyn fmt::Debug| {
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    };
    same(
        &StretchMethodChoice::AutoAsinh.config(),
        &Stretch::auto_asinh(),
    );
    same(&StretchMethodChoice::AutoStf.config(), &Stretch::auto_stf());
    same(
        &ScnrProtection::AverageNeutral.config(),
        &Scnr::average_neutral(1.0),
    );
    same(
        &ScnrProtection::MaximumNeutral.config(),
        &Scnr::maximum_neutral(1.0),
    );
    same(
        &ScnrProtection::MaximumMask.config(),
        &Scnr::maximum_mask(1.0),
    );
    same(
        &ScnrProtection::AdditiveMask.config(),
        &Scnr::additive_mask(1.0),
    );
    assert_eq!(BgMode::Divide.config().mode, BgMode::Divide);
    same(
        &BgMode::Subtract.config(),
        &ExtractBg {
            mode: BgMode::Subtract,
            ..ExtractBg::default()
        },
    );
    for (pick, preset) in [
        (
            DetectionPreset::WideField,
            StarDetectionConfig::wide_field(),
        ),
        (
            DetectionPreset::HighResolution,
            StarDetectionConfig::high_resolution(),
        ),
        (
            DetectionPreset::CrowdedField,
            StarDetectionConfig::crowded_field(),
        ),
        (
            DetectionPreset::PreciseGround,
            StarDetectionConfig::precise_ground(),
        ),
    ] {
        same(&pick.config(), &preset);
    }
    for (pick, preset) in [
        (RegistrationPreset::Default, RegistrationConfig::default()),
        (RegistrationPreset::Fast, RegistrationConfig::fast()),
        (RegistrationPreset::Precise, RegistrationConfig::precise()),
        (
            RegistrationPreset::WideField,
            RegistrationConfig::wide_field(),
        ),
        (RegistrationPreset::Mosaic, RegistrationConfig::mosaic()),
    ] {
        same(&pick.config(), &preset);
    }
    // A pick sets the method; the lights keep their policy.
    for (pick, combine) in [
        (CombineMethodChoice::SigmaClipped, Combine::sigma_clip(3.0)),
        (CombineMethodChoice::Winsorized, Combine::winsorized(3.0)),
        (CombineMethodChoice::Median, Combine::median()),
        (CombineMethodChoice::Mean, Combine::mean()),
    ] {
        assert_eq!(
            pick.config(),
            StackConfig {
                combine,
                ..StackConfig::light()
            },
            "{pick:?}"
        );
    }
}

/// The input runs what it holds: a pick, or a wired config.
#[test]
fn the_input_runs_its_pick_or_its_wired_config() {
    let pick = DynamicValue::from(ConstValue::Enum("auto_stf".to_owned()));
    let picked = StretchMethodChoice::resolve(&pick);
    assert!(matches!(picked.method, StretchMethod::AutoStf { .. }));

    let wired = DynamicValue::from_custom(ConfigValue(StretchKnobs::default()));
    let configured = StretchMethodChoice::resolve(&wired);
    assert!(matches!(configured.method, StretchMethod::AutoAsinh { .. }));
    assert_eq!(configured.color, ColorMode::ColorPreserving);
}
