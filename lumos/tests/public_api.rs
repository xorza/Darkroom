use std::io::{Error, ErrorKind};

use imaginarium::Buffer2;
use lumos::{
    AlignStackError, AlignStackResult, AlignmentSummary, AstroImage, CacheConfig,
    CalibrationComponent, CalibrationMasters, CombineMethod, DefectSummary, DrizzleConfig,
    DrizzleConfigError, DrizzleError, DrizzleFrame, FrameStoreError, GesdConfig, ImageDimensions,
    LinearFitClipConfig, Normalization, PercentileClipConfig, Rejection, SigmaClipConfig, SmallN,
    StackConfig, StackConfigError, StackError, StackProduct, StarDetectionConfig,
    StarDetectionConfigError, StarDetector, StarMatch, Transform, Weighting, WinsorizedClipConfig,
};

#[test]
fn stacking_configuration_types_are_available_from_the_crate_root() {
    let rejections = [
        Rejection::SigmaClip(SigmaClipConfig::default()),
        Rejection::Winsorized(WinsorizedClipConfig::default()),
        Rejection::LinearFit(LinearFitClipConfig::default()),
        Rejection::Percentile(PercentileClipConfig::default()),
        Rejection::Gesd(GesdConfig::default()),
    ];

    let [
        Rejection::SigmaClip(sigma),
        Rejection::Winsorized(winsorized),
        Rejection::LinearFit(linear_fit),
        Rejection::Percentile(percentile),
        Rejection::Gesd(gesd),
    ] = rejections
    else {
        panic!("rejection variants changed order")
    };
    assert_eq!(sigma, SigmaClipConfig::default());
    assert_eq!(winsorized, WinsorizedClipConfig::default());
    assert_eq!(linear_fit, LinearFitClipConfig::default());
    assert_eq!(percentile, PercentileClipConfig::default());
    assert_eq!(gesd, GesdConfig::default());

    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        weighting: Weighting::Manual(vec![1.0, 2.0]),
        normalization: Normalization::Global,
        small_n: SmallN {
            min_frames: 3,
            fallback: CombineMethod::Median,
        },
        cache: CacheConfig::default(),
    };
    let StackConfig {
        method,
        weighting,
        normalization,
        small_n,
        cache,
    } = config;

    assert_eq!(method, CombineMethod::Mean(Rejection::None));
    assert_eq!(weighting, Weighting::Manual(vec![1.0, 2.0]));
    assert_eq!(normalization, Normalization::Global);
    assert_eq!(small_n.min_frames, 3);
    assert_eq!(small_n.fallback, CombineMethod::Median);
    let _: CacheConfig = cache;

    let frame = DrizzleFrame::new("light.fits", Transform::identity());
    let DrizzleFrame {
        source,
        transform: _,
        weight,
        pixel_weight_map,
    } = frame;
    assert_eq!(source, "light.fits");
    assert_eq!(weight, 1.0);
    assert!(pixel_weight_map.is_none());
}

#[test]
fn stacking_configuration_errors_are_available_from_the_crate_root() {
    let stack_error = StackConfig::sigma_clipped(0.0).validate().unwrap_err();
    assert_eq!(
        stack_error,
        StackConfigError::InvalidSigmaLow { value: 0.0 }
    );
    let operation_error: StackError = stack_error.into();
    assert!(matches!(
        operation_error,
        StackError::Config(StackConfigError::InvalidSigmaLow { value: 0.0 })
    ));

    let storage_error = FrameStoreError::CreateDirectory {
        path: ".tmp/unwritable".into(),
        source: Error::new(ErrorKind::PermissionDenied, "denied"),
    };
    let operation_error: StackError = storage_error.into();
    assert_eq!(
        operation_error.to_string(),
        "failed to create frame-store directory '.tmp/unwritable': denied"
    );

    let drizzle_error = DrizzleConfig {
        scale: 0.0,
        ..Default::default()
    }
    .validate()
    .unwrap_err();
    assert_eq!(
        drizzle_error,
        DrizzleConfigError::InvalidScale { value: 0.0 }
    );
    let operation_error: DrizzleError = drizzle_error.into();
    assert!(matches!(
        operation_error,
        DrizzleError::Config(DrizzleConfigError::InvalidScale { value: 0.0 })
    ));

    let detection_error = StarDetector::from_config(StarDetectionConfig {
        sigma_threshold: 0.0,
        ..StarDetectionConfig::default()
    })
    .unwrap_err();
    assert_eq!(
        detection_error,
        StarDetectionConfigError::InvalidSigmaThreshold { value: 0.0 }
    );
    let pipeline_error: AlignStackError = detection_error.into();
    assert!(matches!(
        pipeline_error,
        AlignStackError::DetectionConfig(StarDetectionConfigError::InvalidSigmaThreshold {
            value: 0.0
        })
    ));
}

#[test]
fn calibration_master_views_are_available_from_the_crate_root() {
    let masters = CalibrationMasters::default();
    assert_eq!(masters.components().collect::<Vec<_>>(), Vec::new());
    let summary: Option<DefectSummary> = masters.defect_summary();
    assert_eq!(summary, None);
    assert_eq!(CalibrationComponent::FlatDark.to_string(), "flat-dark");
}

#[test]
fn stacking_outputs_and_relationships_use_named_public_types() {
    let product = StackProduct {
        image: AstroImage::from_pixels(ImageDimensions::new((2, 1), 1), vec![0.25, 0.75]),
        coverage: Buffer2::new(2, 1, vec![1.0, 0.5]),
        weight: Buffer2::new(2, 1, vec![2.0, 1.0]),
        variance: Buffer2::new(2, 1, vec![0.5, 1.0]),
    };
    let result = AlignStackResult {
        product,
        alignment: AlignmentSummary {
            reference: 1,
            registered: 2,
            dropped: vec![0, 3],
        },
    };

    assert_eq!(result.product.image.channel(0).pixels(), &[0.25, 0.75]);
    assert_eq!(result.product.coverage.pixels(), &[1.0, 0.5]);
    assert_eq!(result.product.weight.pixels(), &[2.0, 1.0]);
    assert_eq!(result.product.variance.pixels(), &[0.5, 1.0]);
    assert_eq!(result.alignment.reference, 1);
    assert_eq!(result.alignment.registered, 2);
    assert_eq!(result.alignment.dropped, vec![0, 3]);

    let star_match = StarMatch {
        reference: 4,
        target: 9,
    };
    let StarMatch { reference, target } = star_match;
    assert_eq!(reference, 4);
    assert_eq!(target, 9);
}
