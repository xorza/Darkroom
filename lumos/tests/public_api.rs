use imaginarium::Buffer2;
use lumos::{
    AlignStackResult, AlignmentSummary, AstroImage, CacheConfig, CombineMethod, DrizzleConfig,
    DrizzleConfigError, DrizzleError, DrizzleFrame, GesdConfig, ImageDimensions,
    LinearFitClipConfig, Normalization, PercentileClipConfig, Rejection, SigmaClipConfig, SmallN,
    StackConfig, StackConfigError, StackError, StackProduct, Transform, Weighting,
    WinsorizedClipConfig,
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
}

#[test]
fn stacked_outputs_share_one_public_product_type() {
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
}
