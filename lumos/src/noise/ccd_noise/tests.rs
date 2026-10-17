use super::*;

/// The background where a flat multiplied the sample by `g`. Background 1/4 at gain 1, half of
/// it read noise, all dyadic so exact:
/// - gain 1: 1/4; gain 2: `1/4·2·(½·2 + ½)` = 3/4;
/// - the split matters: all read noise gives `1/4·4` = 1 at gain 2, all sky `1/4·2` = 1/2;
/// - a step of σ 1/4 floors a silent background at `(1/4·2)²` = 1/4 at gain 2.
#[test]
fn the_background_follows_the_flat_gain() {
    let noise = CcdNoise {
        background: BackgroundSplit {
            variance: 0.25,
            read_share: 0.5,
        },
        quantization_variance: 0.0,
        sky: 1.0,
        electrons_per_unit: Some(4.0),
    };
    assert_eq!(noise.at(1.0).background, 0.25);
    assert_eq!(noise.at(2.0).background, 0.75);
    // Above the sky of 1, a unit at gain 2 adds 2/4: x = 3 lies 2 units up, 3/4 + 2·1/2 = 7/4;
    // below the sky only the background remains. A warp's confidence of 2 halves both.
    assert_eq!(noise.at(2.0).variance(3.0), 1.75);
    assert_eq!(noise.at(2.0).variance(0.5), 0.75);
    assert_eq!(noise.at(2.0).over(2.0).variance(3.0), 0.875);
    let no_gain = CcdNoise {
        electrons_per_unit: None,
        ..noise
    };
    for (read_share, expected) in [(1.0, 1.0), (0.0, 0.5)] {
        let split = CcdNoise {
            background: BackgroundSplit {
                variance: 0.25,
                read_share,
            },
            ..no_gain
        };
        assert_eq!(split.at(2.0).background, expected, "ρ {read_share}");
        // With no gain, a sample above the sky adds nothing to the background.
        assert_eq!(split.at(2.0).variance(3.0), expected, "ρ {read_share}");
    }
    let silent = CcdNoise {
        background: BackgroundSplit::unflattened(0.0),
        quantization_variance: 1.0 / 16.0,
        ..no_gain
    };
    assert_eq!(silent.at(2.0).background, 0.25);
}
