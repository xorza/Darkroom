use common::CancelToken;

use crate::stacking::combine::config::{Normalization, StackConfig};
use crate::stacking::combine::stack::stack;
use crate::stacking::progress::ProgressCallback;
use crate::testing::{calibration_dir, init_tracing};

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn test_stack_registered_lights() {
    init_tracing();

    let cal_dir = calibration_dir();

    let registered_dir = cal_dir.join("registered_lights");
    assert!(
        registered_dir.exists(),
        "registered_lights directory not found — run registration tests first"
    );

    let paths =
        common::file_utils::files_with_extensions(&registered_dir, &["tiff", "tif", "fits", "fit"]);
    assert!(!paths.is_empty(), "No registered light frames found");
    println!("Stacking {} registered light frames...", paths.len());

    // Sigma-clipped mean with global normalization — best for registered lights
    let config = StackConfig {
        normalization: Normalization::Global,
        ..StackConfig::sigma_clipped(2.5)
    };

    let stacked = stack(
        &paths,
        config,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .expect("Failed to stack registered lights")
    .image;

    println!(
        "Stacked result: {}x{}x{}",
        stacked.width(),
        stacked.height(),
        stacked.channels()
    );

    let out_path = cal_dir.join("stacked_light.tiff");
    let img: imaginarium::Image = stacked.into();
    img.save_file(&out_path)
        .expect("Failed to save stacked light");
    println!("Saved stacked light: {}", out_path.display());
}
