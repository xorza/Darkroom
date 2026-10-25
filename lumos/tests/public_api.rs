use std::io::{Error, ErrorKind};

use common::{Buffer2, CancelToken};
use imaginarium::{ChannelLayout, PlanarDesc};
use lumos::{
    AlignStackConfig, AlignStackConfigError, AlignStackError, AlignStackResult, BgConfig,
    CalibrationComponent, CalibrationError, CalibrationMasters, CalibrationSet, CfaPattern,
    Combine, CombineMethod, Coverage, DefectSummary, DerivedFacts, DetectionDiagnostics, DomainMap,
    DrizzleConfig, DrizzleConfigError, DrizzleError, DrizzleFrame, ExtractionConfig, FilterConfig,
    FitsChecksumPolicy, FitsChecksumProvenance, FitsChecksumState, FitsCubeInterpretation,
    FitsFloatScale, FitsHduProvenance, FitsHduSelector, FitsLoadConfig, FitsTransferProvenance,
    FlagCounts, FrameRegistration, FrameStoreError, FwhmConfig, GesdConfig, ImageMetadata,
    IngestConfig, InterpolationMethod, InvalidConfigField, LightOutcome, LinearFitClipConfig,
    LinearImage, LinearModel, LoadCtx, MarkesteijnPasses, MasterRole, MatchIdxs, MeasurementConfig,
    Normalization, NullPolicy, ObservationHeader, Pedestal, ProgressMonitor,
    QualityFilterDiagnostics, QualityMap, QualityPlanes, RansacConfig, RegistrationCatalog,
    RegistrationConfig, RegistrationError, RegistrationMatchingConfig, Rejection, RunReport,
    SampleDomain, ScaleOrigin, SigmaClipConfig, SipConfig, SipOrder, SmallN, SourceProvenance,
    StackConfig, StackConfigError, StackError, StackProduct, StarDetectionConfig, StarDetector,
    StarField, StarMatch, Transform, TransformModel, TransformType, TriangleConfig, TrimConfig,
    WarpConfig, WarpTransform, WeightKind, Weighting, WinsorizedClipConfig,
};

#[test]
fn file_loading_policy_is_available_from_the_crate_root() {
    let _: LoadCtx = LoadCtx {
        cancel: CancelToken::never(),
        fits: FitsLoadConfig {
            hdu: FitsHduSelector::Name {
                extname: "SCI".to_owned(),
                extver: Some(2),
            },
            cube: FitsCubeInterpretation::Rgb,
            checksum: FitsChecksumPolicy::RequireValid,
            float_scale: FitsFloatScale::FullScale(65_535.0),
            unstated_bayer_pattern: Some(CfaPattern::Grbg),
            pedestal: Pedestal::Unknown,
        },
        nulls: NullPolicy::Reject,
        xtrans_passes: MarkesteijnPasses::Three,
        decode_threads: 4,
    };
    // The default is the standard-conforming one: a null is data the format defines, not a reason
    // to refuse the file.
    assert_eq!(NullPolicy::default(), NullPolicy::Mask);

    let _ = SourceProvenance::Fits(FitsTransferProvenance {
        bscale: 1.0,
        bzero: 0.0,
        hdu: FitsHduProvenance {
            idx: 3,
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
        Rejection::Trim(TrimConfig::default()),
        Rejection::Gesd(GesdConfig::default()),
    ];

    let _: StackConfig = StackConfig {
        combine: Combine {
            method: CombineMethod::Mean(Rejection::None),
            small_n: SmallN {
                min_frames: 3,
                fallback: CombineMethod::Median,
            },
        },
        weighting: Weighting::Manual(vec![1.0, 2.0]),
        normalization: Normalization::Global,
        quality: QualityPlanes::IMAGE_ONLY,
        min_survivors: 4,
    };
    let _: IngestConfig = IngestConfig {
        keep_cache: true,
        ..IngestConfig::with_cache_dir("cache".into())
    };
    assert_eq!(QualityPlanes::default(), QualityPlanes::STANDARD);
    assert_eq!(
        QualityPlanes::ALL,
        QualityPlanes {
            dispersion: true,
            ..QualityPlanes::STANDARD
        }
    );

    let registration = RegistrationConfig {
        model: TransformModel::FixedWithSip(
            LinearModel::Similarity,
            SipConfig {
                order: SipOrder::Two,
                ..Default::default()
            },
        ),
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
            seed: 42,
            ..Default::default()
        },
        ..Default::default()
    };
    registration.registrar().unwrap();
    let align = AlignStackConfig {
        registration,
        warp: WarpConfig {
            method: InterpolationMethod::Bilinear,
            border_value: -1.0,
            clamping_threshold: None,
        },
        ..Default::default()
    };
    let registration = &align.registration;
    // `min_stars` overrides the transform's own floor.
    assert_eq!(registration.matching.required_stars(registration.model), 10);

    let detection = StarDetectionConfig {
        bg: BgConfig::default(),
        extraction: ExtractionConfig::default(),
        fwhm: FwhmConfig::default(),
        measurement: MeasurementConfig::default(),
        filter: FilterConfig::default(),
    };
    StarDetector::from_config(detection).unwrap();

    // A drizzle frame has no per-pixel weights by default; its weight is the combine's.
    let frame = DrizzleFrame::new("light.fits", WarpTransform::new(Transform::identity()));
    assert!(frame.pixel_weight_map.is_none());
}

#[test]
fn invariant_types_expose_validated_state_from_the_crate_root() {
    let _: ImageMetadata = ImageMetadata {
        header: ObservationHeader {
            camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
            ..Default::default()
        },
        derived: DerivedFacts::default(),
    };

    let desc = PlanarDesc::new((12, 8), ChannelLayout::Rgb);
    assert_eq!(desc.pixel_count(), 96);
    assert_eq!(desc.sample_count(), 288);

    let transform = Transform::similarity(glam::DVec2::new(3.0, -2.0), 0.25, 1.1);
    assert_eq!(transform.transform_type(), TransformType::Similarity);
    assert_eq!(transform.matrix()[8], 1.0);
}

#[test]
fn stacking_configuration_errors_are_available_from_the_crate_root() {
    let frame =
        LinearImage::from_interleaved(PlanarDesc::new((2, 2), ChannelLayout::Mono), vec![0.5; 4]);
    let operation_error = StackConfig {
        combine: Combine::sigma_clip(0.0),
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    }
    .stack_images(vec![frame.into()], ProgressMonitor::unattended())
    .unwrap_err();
    assert!(
        matches!(
            &operation_error,
            StackError::Config(StackConfigError::Field(invalid))
                if *invalid == InvalidConfigField {
                    field: "sigma.low",
                    expected: "finite and positive",
                    value: 0.0,
                    bound: None,
                }
        ),
        "{operation_error:?}"
    );
    assert_eq!(
        operation_error.to_string(),
        "sigma.low must be finite and positive, got 0"
    );

    let storage_error = FrameStoreError::CreateDir {
        path: ".tmp/unwritable".into(),
        source: Error::new(ErrorKind::PermissionDenied, "denied"),
    };
    let operation_error: StackError = storage_error.into();
    assert_eq!(
        operation_error.to_string(),
        "failed to create frame-store directory '.tmp/unwritable': denied"
    );

    // Refused before the frame is read: the file need not exist.
    let operation_error = DrizzleConfig {
        scale: 0.0,
        ..Default::default()
    }
    .drizzle_paths(
        vec![DrizzleFrame::new(
            "light.fits",
            WarpTransform::new(Transform::identity()),
        )],
        &IngestConfig::default(),
        ProgressMonitor::unattended(),
    )
    .unwrap_err();
    assert!(
        matches!(
            &operation_error,
            DrizzleError::Config(DrizzleConfigError::Field(invalid))
                if *invalid == InvalidConfigField {
                    field: "scale",
                    expected: "finite and positive",
                    value: 0.0,
                    bound: None,
                }
        ),
        "{operation_error:?}"
    );
    assert_eq!(
        operation_error.to_string(),
        "scale must be finite and positive, got 0"
    );

    let detection_error = StarDetector::from_config(StarDetectionConfig {
        extraction: ExtractionConfig {
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
    let pipeline_error = AlignStackError::Config(AlignStackConfigError::Detection(detection_error));
    assert_eq!(
        pipeline_error.to_string(),
        "invalid configuration: star detection: sigma_threshold must be finite and positive, got 0"
    );

    let pipeline_error = AlignStackError::Calibration {
        path: "light_7.cr2".into(),
        source: Box::new(CalibrationError::AlreadyCalibrated),
    };
    assert_eq!(
        pipeline_error.to_string(),
        "cannot calibrate light frame 'light_7.cr2': the light frame is already calibrated"
    );

    let registration_error = RegistrationError::InvalidStarField {
        catalog: RegistrationCatalog::Target,
        idx: 7,
        field: StarField::Fwhm,
        value: f64::INFINITY,
    };
    assert_eq!(
        registration_error.to_string(),
        "target star 7 FWHM must be finite, got inf"
    );
}

#[test]
fn star_detection_filter_diagnostics_are_one_nested_component() {
    let quality_filter = QualityFilterDiagnostics {
        saturated: 1,
        low_snr: 2,
        high_eccentricity: 3,
        cosmic_rays: 4,
        roundness: 5,
        fwhm_outliers: 6,
        duplicates: 7,
    };
    let _: DetectionDiagnostics = DetectionDiagnostics {
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
        image: LinearImage::from_interleaved(
            PlanarDesc::new((2, 1), ChannelLayout::Mono),
            vec![0.25, 0.75],
        ),
        coverage: Some(Coverage::PerPixel(QualityMap::Shared(Buffer2::new(
            (2, 1),
            vec![1.0, 0.5],
        )))),
        weight: Some(QualityMap::Shared(Buffer2::new((2, 1), vec![2.0, 1.0]))),
        weight_kind: WeightKind::InverseVariance,
        inverse_variance: Some(QualityMap::Shared(Buffer2::new((2, 1), vec![2.0, 1.0]))),
        dispersion: Some(QualityMap::Shared(Buffer2::new(
            (2, 1),
            vec![0.6, f32::NAN],
        ))),
        cfa_type: None,
        report: RunReport {
            excluded_samples: FlagCounts::default(),
            kept_flagged_samples: FlagCounts::default(),
            variance_bg_only: true,
            ..RunReport::default()
        },
    };
    let _: AlignStackResult = AlignStackResult {
        product,
        reference: 1,
        lights: [
            FrameRegistration::Dropped(RegistrationError::NoMatchingPatterns),
            FrameRegistration::Reference,
            FrameRegistration::Registered {
                warp: Box::new(WarpTransform::new(Transform::identity())),
                inliers: 40,
                rms_error: 0.1,
            },
            FrameRegistration::Dropped(RegistrationError::NoMatchingPatterns),
        ]
        .into_iter()
        .map(|registration| LightOutcome {
            detection: DetectionDiagnostics::default(),
            registration,
        })
        .collect(),
    };

    // A uniform coverage is one number until a plane is asked for.
    let uniform = Coverage::Uniform {
        value: 0.5,
        size: (2, 1).into(),
    };
    assert_eq!(uniform.to_plane(0).pixels(), &[0.5, 0.5]);

    // The conversions to an image move the planes rather than copy them.
    let shared_plane = Buffer2::new((2, 1), vec![3.0, 4.0]);
    let shared_pixels = shared_plane.pixels().as_ptr();
    let shared_image = LinearImage::from(QualityMap::Shared(shared_plane));
    assert_eq!(
        shared_image.desc(),
        PlanarDesc::new((2, 1), ChannelLayout::Mono)
    );
    assert_eq!(shared_image.channel(0).pixels(), &[3.0, 4.0]);
    assert_eq!(shared_image.channel(0).pixels().as_ptr(), shared_pixels);

    let per_channel_planes = [
        Buffer2::new((1, 1), vec![5.0]),
        Buffer2::new((1, 1), vec![6.0]),
        Buffer2::new((1, 1), vec![7.0]),
    ];
    let per_channel_pixels = per_channel_planes[1].pixels().as_ptr();
    let per_channel_map = QualityMap::PerChannel(per_channel_planes);
    let per_channel_image = LinearImage::from(per_channel_map);
    assert_eq!(
        per_channel_image.desc(),
        PlanarDesc::new((1, 1), ChannelLayout::Rgb)
    );
    assert_eq!(per_channel_image.channel(0).pixels(), &[5.0]);
    assert_eq!(per_channel_image.channel(1).pixels(), &[6.0]);
    assert_eq!(per_channel_image.channel(2).pixels(), &[7.0]);
    assert_eq!(
        per_channel_image.channel(1).pixels().as_ptr(),
        per_channel_pixels
    );

    let _: StarMatch = StarMatch {
        idxs: MatchIdxs {
            reference: 4,
            target: 9,
        },
        residual: 0.125,
    };
}
