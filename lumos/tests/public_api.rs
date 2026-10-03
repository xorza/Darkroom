use std::io::{Error, ErrorKind};

use common::CancelToken;
use imaginarium::Buffer2;
use lumos::{
    AlignStackError, AlignStackResult, AlignmentSummary, CacheConfig, CalibrationComponent,
    CalibrationError, CalibrationMasters, CalibrationSet, CfaPattern, CombineMethod, Coverage,
    DefectSummary, DomainMap, DrizzleConfig, DrizzleConfigError, DrizzleError, DrizzleFrame,
    FitsChecksumPolicy, FitsChecksumProvenance, FitsChecksumState, FitsCubeInterpretation,
    FitsFloatScale, FitsHduProvenance, FitsHduSelector, FitsLoadOptions, FitsNullPolicy,
    FitsTransferProvenance, FrameStoreError, GesdConfig, ImageDimensions, ImageMetadata,
    InterpolationMethod, InvalidConfigField, LinearFitClipConfig, LinearImage, LoadContext,
    MasterRole, MatchIndices, NoiseModel, Normalization, Pedestal, PercentileClipConfig,
    QualityMap, QualityPlanes, RansacConfig, RegistrationCatalog, RegistrationConfig,
    RegistrationError, RegistrationMatchingConfig, Rejection, SampleDomain, ScaleOrigin,
    SigmaClipConfig, SipConfig, SmallN, StackConfig, StackConfigError, StackError, StackProduct,
    StarDetectionBackgroundConfig, StarDetectionCandidateConfig, StarDetectionConfig,
    StarDetectionDiagnostics, StarDetectionFilterConfig, StarDetectionFwhmConfig,
    StarDetectionMeasurementConfig, StarDetectionQualityFilterDiagnostics, StarDetector, StarMatch,
    TransferProvenance, Transform, TransformModel, TransformType, TriangleConfig, WarpParams,
    WarpTransform, Weighting, WinsorizedClipConfig,
};

#[test]
fn file_loading_policy_is_available_from_the_crate_root() {
    let _: LoadContext = LoadContext {
        cancel: CancelToken::never(),
        memory_limit_bytes: 64 * 1024 * 1024,
        fits: FitsLoadOptions {
            hdu: FitsHduSelector::Name {
                extname: "SCI".to_owned(),
                extver: Some(2),
            },
            cube: FitsCubeInterpretation::Rgb,
            checksum: FitsChecksumPolicy::RequireValid,
            float_scale: FitsFloatScale::FullScale(65_535.0),
            nulls: FitsNullPolicy::Reject,
            unstated_bayer_pattern: Some(CfaPattern::Grbg),
            pedestal: Pedestal::Unknown,
        },
    };
    // The default is the standard-conforming one: a null is data the format defines, not a reason
    // to refuse the file.
    assert_eq!(FitsLoadOptions::default().nulls, FitsNullPolicy::Mask);

    let _ = TransferProvenance::FitsNormalized(FitsTransferProvenance {
        bscale: 1.0,
        bzero: 0.0,
        hdu: FitsHduProvenance {
            index: 3,
            extname: Some("SCI".to_owned()),
            extver: Some(2),
        },
        checksum: FitsChecksumProvenance {
            datasum: FitsChecksumState::Valid,
            checksum: FitsChecksumState::Valid,
        },
    });
    // A caller compares two frames' domains without knowing which decoder produced them. A FITS
    // frame that kept a 2048-ADU pedestal relates to a RAW frame with none by the scale ratio and an
    // offset that moves the pedestal away.
    let fits = SampleDomain {
        scale: 65_535.0,
        origin: ScaleOrigin::Declared,
        pedestal: Pedestal::Kept(2048.0),
        unit: Some("adu".to_owned()),
    };
    let raw = SampleDomain {
        scale: 16_383.0,
        origin: ScaleOrigin::Declared,
        pedestal: Pedestal::Removed,
        unit: None,
    };
    assert_eq!(
        fits.conversion_to(&raw),
        Some(DomainMap {
            gain: 65_535.0 / 16_383.0,
            offset: -2048.0 / 16_383.0,
        })
    );
}

#[test]
fn stacking_configuration_types_are_available_from_the_crate_root() {
    let _: [Rejection; 5] = [
        Rejection::SigmaClip(SigmaClipConfig::default()),
        Rejection::Winsorized(WinsorizedClipConfig::default()),
        Rejection::LinearFit(LinearFitClipConfig::default()),
        Rejection::Percentile(PercentileClipConfig::default()),
        Rejection::Gesd(GesdConfig::default()),
    ];

    let _: StackConfig = StackConfig {
        method: CombineMethod::Mean(Rejection::None),
        weighting: Weighting::Manual(vec![1.0, 2.0]),
        normalization: Normalization::Global,
        small_n: SmallN {
            min_frames: 3,
            fallback: CombineMethod::Median,
        },
        cache: CacheConfig::default(),
        quality: QualityPlanes::IMAGE_ONLY,
    };
    assert_eq!(QualityPlanes::default(), QualityPlanes::ALL);

    let registration = RegistrationConfig {
        transform_type: TransformModel::Fixed(TransformType::Similarity),
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
    // `min_stars` overrides the transform's own floor.
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

    NoiseModel::from_normalized(1_000.0, 10.0)
        .validate()
        .unwrap();

    // A drizzle frame weighs one by default and has no per-pixel weights.
    let frame = DrizzleFrame::new("light.fits", WarpTransform::new(Transform::identity()));
    assert_eq!(frame.weight, 1.0);
    assert!(frame.pixel_weight_map.is_none());
}

#[test]
fn invariant_types_expose_validated_state_from_the_crate_root() {
    let _: ImageMetadata = ImageMetadata {
        camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
        ..Default::default()
    };

    let dimensions = ImageDimensions::new((12, 8), 3);
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
        StackConfigError::Field(InvalidConfigField {
            field: "sigma_low",
            expected: "finite and positive",
            value: 0.0,
            bound: None,
        })
    );
    let operation_error: StackError = stack_error.into();
    assert_eq!(
        operation_error.to_string(),
        "sigma_low must be finite and positive, got 0"
    );

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
        DrizzleConfigError::Field(InvalidConfigField {
            field: "scale",
            expected: "finite and positive",
            value: 0.0,
            bound: None,
        })
    );
    let operation_error: DrizzleError = drizzle_error.into();
    assert_eq!(
        operation_error.to_string(),
        "scale must be finite and positive, got 0"
    );

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
        InvalidConfigField {
            field: "sigma_threshold",
            expected: "finite and positive",
            value: 0.0,
            bound: None,
        }
    );
    let pipeline_error = AlignStackError::DetectionConfig(detection_error);
    assert_eq!(
        pipeline_error.to_string(),
        "invalid star-detection configuration: sigma_threshold must be finite and positive, got 0"
    );

    let calibration_error = CalibrationError::AlreadyCalibrated;
    let pipeline_error: AlignStackError = calibration_error.into();
    assert!(matches!(
        pipeline_error,
        AlignStackError::Calibration(CalibrationError::AlreadyCalibrated)
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
    let _: StarDetectionDiagnostics = StarDetectionDiagnostics {
        quality_filter,
        ..Default::default()
    };
}

#[test]
fn calibration_master_views_are_available_from_the_crate_root() {
    let _: CalibrationSet<u8> = CalibrationSet {
        dark: 1,
        flat: 2,
        bias: 3,
        flat_dark: 4,
    };

    let masters = CalibrationMasters::default();
    assert_eq!(masters.components().collect::<Vec<_>>(), Vec::new());
    let summary: Option<DefectSummary> = masters.defect_summary();
    assert_eq!(summary, None);
    assert_eq!(
        CalibrationComponent::Master(MasterRole::FlatDark).to_string(),
        "flat-dark"
    );
}

#[test]
fn stacking_outputs_and_relationships_use_named_public_types() {
    let product = StackProduct {
        image: LinearImage::from_pixels(ImageDimensions::new((2, 1), 1), vec![0.25, 0.75]),
        coverage: Some(Coverage::PerPixel(Buffer2::new(2, 1, vec![1.0, 0.5]))),
        weight: Some(QualityMap::Shared(Buffer2::new(2, 1, vec![2.0, 1.0]))),
        linear_variance: Some(QualityMap::Shared(Buffer2::new(2, 1, vec![0.5, 1.0]))),
        cfa_type: None,
    };
    let _: AlignStackResult = AlignStackResult {
        product,
        alignment: AlignmentSummary {
            reference: 1,
            registered: 2,
            dropped: vec![0, 3],
        },
        detection: vec![StarDetectionDiagnostics::default(); 4],
    };

    // A uniform coverage is one number until a plane is asked for.
    let uniform = Coverage::Uniform {
        value: 0.5,
        size: (2, 1).into(),
    };
    assert_eq!(uniform.to_plane().pixels(), &[0.5, 0.5]);

    // The conversions to an image move the planes rather than copy them.
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

    let _: StarMatch = StarMatch {
        indices: MatchIndices {
            reference: 4,
            target: 9,
        },
        residual: 0.125,
    };
}
