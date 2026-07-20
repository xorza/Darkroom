use crate::image_ops::rgb::Rgb;

#[test]
fn intensity_scale_and_zero_have_exact_channel_values() {
    let color = Rgb {
        r: 0.3,
        g: 0.6,
        b: 0.9,
    };

    assert!((color.intensity() - 0.6).abs() < f32::EPSILON);
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
