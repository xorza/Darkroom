use crate::stacking::{
    FrameType, Normalization, ProgressCallback, StackConfig, stack_with_progress,
};
use crate::testing::{calibration_dir, init_tracing};

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR with registered_lights/
fn test_stack_registered_lights() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        return;
    };

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

    let stacked = stack_with_progress(
        &paths,
        FrameType::Light,
        config,
        ProgressCallback::default(),
    )
    .expect("Failed to stack registered lights");

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
