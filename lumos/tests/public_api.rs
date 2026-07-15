use lumos::{
    CacheConfig, CombineMethod, GesdConfig, LinearFitClipConfig, Normalization,
    PercentileClipConfig, Rejection, SigmaClipConfig, SmallN, StackConfig, Weighting,
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
}
