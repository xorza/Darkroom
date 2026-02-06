use crate::stacking::{FrameType, ProgressCallback, StackConfig, stack_with_progress};
use crate::testing::{calibration_dir, calibration_image_paths, init_tracing};

#[test]
#[ignore] // Requires LUMOS_CALIBRATION_DIR
fn test_create_master_dark_and_flat() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        return;
    };

    let masters_dir = cal_dir.join("calibration_masters");
    std::fs::create_dir_all(&masters_dir).expect("Failed to create calibration_masters dir");

    // Stack darks
    let Some(dark_paths) = calibration_image_paths("Darks") else {
        eprintln!("No Darks directory found, skipping");
        return;
    };
    assert!(!dark_paths.is_empty(), "No dark frames found");

    println!("Stacking {} dark frames...", dark_paths.len());
    let config = StackConfig::sigma_clipped(3.0);
    let master_dark = stack_with_progress(
        &dark_paths,
        FrameType::Dark,
        config,
        ProgressCallback::default(),
    )
    .expect("Failed to stack darks");

    println!(
        "Master dark: {}x{}x{}",
        master_dark.width(),
        master_dark.height(),
        master_dark.channels()
    );

    let first_dark = crate::AstroImage::from_file(&dark_paths[0]).unwrap();
    assert_eq!(master_dark.dimensions(), first_dark.dimensions());

    let dark_path = masters_dir.join("master_dark_mean.tiff");
    let img: imaginarium::Image = master_dark.into();
    img.save_file(&dark_path)
        .expect("Failed to save master dark");
    println!("Saved master dark: {}", dark_path.display());

    // Stack flats
    let Some(flat_paths) = calibration_image_paths("Flats") else {
        eprintln!("No Flats directory found, skipping flats");
        return;
    };
    assert!(!flat_paths.is_empty(), "No flat frames found");

    println!("Stacking {} flat frames...", flat_paths.len());
    let config = StackConfig::sigma_clipped(2.5);
    let master_flat = stack_with_progress(
        &flat_paths,
        FrameType::Flat,
        config,
        ProgressCallback::default(),
    )
    .expect("Failed to stack flats");

    println!(
        "Master flat: {}x{}x{}",
        master_flat.width(),
        master_flat.height(),
        master_flat.channels()
    );

    let first_flat = crate::AstroImage::from_file(&flat_paths[0]).unwrap();
    assert_eq!(master_flat.dimensions(), first_flat.dimensions());

    let flat_path = masters_dir.join("master_flat_mean.tiff");
    let img: imaginarium::Image = master_flat.into();
    img.save_file(&flat_path)
        .expect("Failed to save master flat");
    println!("Saved master flat: {}", flat_path.display());
}
