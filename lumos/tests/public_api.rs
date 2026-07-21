use std::io::{Error, ErrorKind};

use common::CancelToken;
use imaginarium::Buffer2;
use lumos::{
    AlignStackError, AlignStackResult, AlignmentSummary, CacheConfig, CalibrationComponent,
    CalibrationError, CalibrationMasters, CalibrationSet, CombineMethod, DefectSummary,
    DrizzleConfig, DrizzleConfigError, DrizzleError, DrizzleFrame, FitsChecksumPolicy,
    FitsChecksumProvenance, FitsChecksumState, FitsCubeInterpretation, FitsHduProvenance,
    FitsHduSelector, FitsLoadOptions, FrameStoreError, GesdConfig, ImageDimensions, ImageMetadata,
    InterpolationMethod, LinearFitClipConfig, LinearImage, LoadContext, NoiseModel, Normalization,
    PercentileClipConfig, QualityMap, RansacConfig, RegistrationCatalog, RegistrationConfig,
    RegistrationError, RegistrationMatchingConfig, Rejection, SigmaClipConfig, SipConfig, SmallN,
    StackConfig, StackConfigError, StackError, StackProduct,
    StarDetectionBackgroundConfig, StarDetectionCandidateConfig, StarDetectionConfig,
    StarDetectionConfigError, StarDetectionDiagnostics, StarDetectionFilterConfig,
    StarDetectionFwhmConfig, StarDetectionMeasurementConfig, StarDetectionQualityFilterDiagnostics,
    StarDetector, StarMatch, TransferProvenance, Transform, TransformType, TriangleConfig,
    WarpParams, Weighting, WinsorizedClipConfig,
};

#[test]
fn file_loading_policy_is_available_from_the_crate_root() {
    let context = LoadContext {
        cancel: CancelToken::never(),
        memory_limit_bytes: 64 * 1024 * 1024,
        fits: FitsLoadOptions {
            hdu: FitsHduSelector::Name {
                extname: "SCI".to_owned(),
                extver: Some(2),
            },
            cube: FitsCubeInterpretation::Rgb,
            checksum: FitsChecksumPolicy::RequireValid,
        },
    };
    assert_eq!(context.memory_limit_bytes, 64 * 1024 * 1024);
    assert_eq!(context.fits.cube, FitsCubeInterpretation::Rgb);

    let provenance = TransferProvenance::FitsPhysical {
        bscale: 1.0,
        bzero: 0.0,
        unit: Some("adu".to_owned()),
        hdu: FitsHduProvenance {
            index: 3,
            extname: Some("SCI".to_owned()),
            extver: Some(2),
        },
        checksum: FitsChecksumProvenance {
            datasum: FitsChecksumState::Valid,
            checksum: FitsChecksumState::Valid,
        },
    };
    assert!(matches!(
        provenance,
        TransferProvenance::FitsPhysical {
            hdu: FitsHduProvenance { index: 3, .. },
            checksum: FitsChecksumProvenance {
                datasum: FitsChecksumState::Valid,
                checksum: FitsChecksumState::Valid,
            },
            ..
        }
    ));
}

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

    let registration = RegistrationConfig {
        transform_type: TransformType::Similarity,
        matching: RegistrationMatchingConfig {
            max_stars: 50,
            min_stars: Some(10),
            min_matches: 6,
            triangle: TriangleConfig {
                ratio_tolerance: 0.02,
                min_votes: 4,
                check_orientation: false,
            },
        },
        ransac: RansacConfig {
            max_iterations: 750,
            seed: Some(42),
            ..Default::default()
        },
        sip: Some(SipConfig {
            order: 2,
            ..Default::default()
        }),
        warp: WarpParams {
            method: InterpolationMethod::Bilinear,
            border_value: -1.0,
        },
        ..Default::default()
    };
    registration.validate().unwrap();
    assert_eq!(registration.matching.max_stars, 50);
    assert_eq!(registration.matching.min_matches, 6);
    assert_eq!(registration.matching.triangle.ratio_tolerance, 0.02);
    assert_eq!(registration.ransac.max_iterations, 750);
    assert_eq!(registration.ransac.seed, Some(42));
    assert_eq!(registration.sip.as_ref().unwrap().order, 2);
    assert_eq!(registration.warp.method, InterpolationMethod::Bilinear);
    assert_eq!(registration.warp.border_value, -1.0);
    assert_eq!(
        registration
            .matching
            .required_stars(registration.transform_type),
        10
    );

    let detection = StarDetectionConfig {
        background: StarDetectionBackgroundConfig::default(),
        detection: StarDetectionCandidateConfig::default(),
        fwhm: StarDetectionFwhmConfig::default(),
        measurement: StarDetectionMeasurementConfig::default(),
        filter: StarDetectionFilterConfig::default(),
    };
    detection.validate().unwrap();

    let noise = NoiseModel::from_normalized(1_000.0, 10.0);
    assert_eq!(noise.electrons_per_normalized_unit, 1_000.0);
    assert_eq!(noise.read_noise_electrons, 10.0);
    noise.validate().unwrap();

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
fn invariant_types_expose_validated_state_from_the_crate_root() {
    let metadata = ImageMetadata {
        camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
        ..Default::default()
    };
    assert_eq!(metadata.camera_white_balance, Some([2.0, 1.0, 1.5, 1.0]));

    let dimensions = ImageDimensions::new((12, 8), 3);
    assert_eq!(dimensions.size(), (12, 8).into());
    assert_eq!(dimensions.channels(), 3);
    assert_eq!(dimensions.pixel_count(), 96);
    assert_eq!(dimensions.sample_count(), 288);

    let transform = Transform::similarity(glam::DVec2::new(3.0, -2.0), 0.25, 1.1);
    assert_eq!(transform.transform_type(), TransformType::Similarity);
    assert_eq!(transform.matrix()[8], 1.0);
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
        detection: StarDetectionCandidateConfig {
            sigma_threshold: 0.0,
            ..Default::default()
        },
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

    let calibration_error = CalibrationError::MissingLightCfaPattern;
    let pipeline_error: AlignStackError = calibration_error.into();
    assert!(matches!(
        pipeline_error,
        AlignStackError::Calibration(CalibrationError::MissingLightCfaPattern)
    ));

    let registration_error = RegistrationError::InvalidStarFwhm {
        catalog: RegistrationCatalog::Target,
        index: 7,
        value: f32::INFINITY,
    };
    assert_eq!(
        registration_error.to_string(),
        "target star 7 FWHM must be finite, got inf"
    );
}

#[test]
fn star_detection_filter_diagnostics_are_one_nested_component() {
    let quality_filter = StarDetectionQualityFilterDiagnostics {
        saturated: 1,
        low_snr: 2,
        high_eccentricity: 3,
        cosmic_rays: 4,
        roundness: 5,
        fwhm_outliers: 6,
        duplicates: 7,
    };
    let diagnostics = StarDetectionDiagnostics {
        quality_filter,
        ..Default::default()
    };

    assert_eq!(diagnostics.quality_filter, quality_filter);
}

#[test]
fn calibration_master_views_are_available_from_the_crate_root() {
    let roles = CalibrationSet {
        dark: 1,
        flat: 2,
        bias: 3,
        flat_dark: 4,
    };
    assert_eq!(
        [roles.dark, roles.flat, roles.bias, roles.flat_dark],
        [1, 2, 3, 4]
    );

    let masters = CalibrationMasters::default();
    assert_eq!(masters.components().collect::<Vec<_>>(), Vec::new());
    let summary: Option<DefectSummary> = masters.defect_summary();
    assert_eq!(summary, None);
    assert_eq!(CalibrationComponent::FlatDark.to_string(), "flat-dark");
}

#[test]
fn stacking_outputs_and_relationships_use_named_public_types() {
    let product = StackProduct {
        image: LinearImage::from_pixels(ImageDimensions::new((2, 1), 1), vec![0.25, 0.75]),
        coverage: Buffer2::new(2, 1, vec![1.0, 0.5]),
        weight: QualityMap::Shared(Buffer2::new(2, 1, vec![2.0, 1.0])),
        linear_variance: Some(QualityMap::Shared(Buffer2::new(2, 1, vec![0.5, 1.0]))),
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
    assert_eq!(result.product.weight.channel(0).pixels(), &[2.0, 1.0]);
    assert_eq!(
        result
            .product
            .linear_variance
            .as_ref()
            .unwrap()
            .channel(0)
            .pixels(),
        &[0.5, 1.0]
    );
    assert_eq!(result.alignment.reference, 1);
    assert_eq!(result.alignment.registered, 2);
    assert_eq!(result.alignment.dropped, vec![0, 3]);

    let shared_plane = Buffer2::new(2, 1, vec![3.0, 4.0]);
    let shared_pixels = shared_plane.pixels().as_ptr();
    let shared_image = LinearImage::from(QualityMap::Shared(shared_plane));
    assert_eq!(shared_image.dimensions(), ImageDimensions::new((2, 1), 1));
    assert_eq!(shared_image.channel(0).pixels(), &[3.0, 4.0]);
    assert_eq!(shared_image.channel(0).pixels().as_ptr(), shared_pixels);

    let per_channel_planes = [
        Buffer2::new(1, 1, vec![5.0]),
        Buffer2::new(1, 1, vec![6.0]),
        Buffer2::new(1, 1, vec![7.0]),
    ];
    let per_channel_pixels = per_channel_planes[1].pixels().as_ptr();
    let per_channel_map = QualityMap::PerChannel(per_channel_planes);
    let per_channel_image = LinearImage::from(per_channel_map);
    assert_eq!(
        per_channel_image.dimensions(),
        ImageDimensions::new((1, 1), 3)
    );
    assert_eq!(per_channel_image.channel(0).pixels(), &[5.0]);
    assert_eq!(per_channel_image.channel(1).pixels(), &[6.0]);
    assert_eq!(per_channel_image.channel(2).pixels(), &[7.0]);
    assert_eq!(
        per_channel_image.channel(1).pixels().as_ptr(),
        per_channel_pixels
    );

    let star_match = StarMatch {
        reference: 4,
        target: 9,
        residual: 0.125,
    };
    let StarMatch {
        reference,
        target,
        residual,
    } = star_match;
    assert_eq!(reference, 4);
    assert_eq!(target, 9);
    assert_eq!(residual, 0.125);
}
