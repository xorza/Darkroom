use crate::image_ops::rgb::Rgb;
use crate::internals::assertions::assert_close;

#[test]
fn intensity_scale_and_zero_have_exact_channel_values() {
    let color = Rgb {
        r: 0.3,
        g: 0.6,
        b: 0.9,
    };

    // (0.3 + 0.6 + 0.9)/3 rounds the two sums and the division: within ε of 0.6.
    assert_close!(color.intensity(), 0.6, f32::EPSILON);
    assert_eq!(
        color.scale(2.0),
        Rgb {
            r: 0.6,
            g: 1.2,
            b: 1.8,
        }
    );
    assert_eq!(
        Rgb::ZERO,
        Rgb {
            r: 0.0,
            g: 0.0,
            b: 0.0,
        }
    );
}

/// `with_intensity` in its four regimes, on (0.5, 0.25, 0.25) of intensity 1/3 and its kin: to 2/3
/// every channel doubles, `(2/3)/(1/3)` being 2 exactly; to 1 the red would pass white at 1.5, so
/// all three divide by it — (1, 0.5, 0.5), the hue kept where a clip would give (1, 0.75, 0.75); a
/// channel below black beside a positive intensity clamps to 0; and no positive intensity is black.
#[test]
fn with_intensity_keeps_the_hue_in_display_range() {
    let px = Rgb {
        r: 0.5,
        g: 0.25,
        b: 0.25,
    };
    let third = px.intensity();
    assert_eq!(px.with_intensity(2.0 * third), px.scale(2.0));
    let capped = px.with_intensity(3.0 * third);
    assert_eq!((capped.r, capped.g / capped.r), (1.0, 0.5));
    assert_eq!(capped.g, capped.b);

    let negative = Rgb {
        r: 0.6,
        g: -0.1,
        b: 0.1,
    };
    assert_eq!(negative.with_intensity(negative.intensity()).g, 0.0);
    for dark in [Rgb::ZERO, negative.scale(-1.0)] {
        assert_eq!(dark.with_intensity(0.5), Rgb::ZERO, "{dark:?}");
    }
}
