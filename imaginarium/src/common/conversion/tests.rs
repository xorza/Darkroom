use super::conversion_scalar::ChannelConvert;

macro_rules! test_identity {
    ($name:ident, $t:ty, $vals:expr) => {
        #[test]
        fn $name() {
            for v in $vals {
                let converted: $t = v.convert();
                assert_eq!(v, converted, "identity conversion failed for {:?}", v);
            }
        }
    };
}

test_identity!(identity_u8, u8, [0u8, 128, 255]);
test_identity!(identity_u16, u16, [0u16, 32768, 65535]);
test_identity!(identity_f32, f32, [0.0f32, 0.5, 1.0]);

macro_rules! test_upscale_downscale {
    ($name:ident, $small:ty, $large:ty) => {
        #[test]
        fn $name() {
            // Test that upscale then downscale preserves value
            let vals: [$small; 3] = [0, <$small>::MAX / 2, <$small>::MAX];
            for &v in &vals {
                let up: $large = v.convert();
                let down: $small = up.convert();
                assert_eq!(v, down, "upscale->downscale failed for {}", v);
            }
        }
    };
}

// Unsigned upscale/downscale
test_upscale_downscale!(upscale_downscale_u8_u16, u8, u16);

macro_rules! test_float_conversion {
    ($name:ident, $int:ty, $float:ty) => {
        #[test]
        fn $name() {
            // Test boundary values
            let zero: $float = (0 as $int).convert();
            assert!(
                (zero - 0.0).abs() < 0.001,
                "zero conversion failed: got {:?}",
                zero
            );

            let max: $float = <$int>::MAX.convert();
            assert!(
                (max - 1.0).abs() < 0.01,
                "max conversion failed: got {:?}",
                max
            );

            // Test round-trip for normalized values
            let test_floats: [$float; 3] = [0.0, 0.5, 1.0];
            for &f in &test_floats {
                let as_int: $int = f.convert();
                let back: $float = as_int.convert();
                assert!(
                    (f - back).abs() < 0.02,
                    "float round-trip failed for {}: got {}",
                    f,
                    back
                );
            }
        }
    };
}

// Float conversions for unsigned integers
test_float_conversion!(float_conversion_u8_f32, u8, f32);
test_float_conversion!(float_conversion_u16_f32, u16, f32);

// Test that opaque_alpha is correct for each channel type
#[test]
fn opaque_alpha_values() {
    use super::conversion_scalar::OpaqueAlpha;
    assert_eq!(
        f32::opaque_alpha(),
        1.0,
        "f32::opaque_alpha() should be 1.0"
    );
    assert_eq!(u8::opaque_alpha(), 255, "u8::opaque_alpha() should be 255");
    assert_eq!(
        u16::opaque_alpha(),
        65535,
        "u16::opaque_alpha() should be 65535"
    );
}

// Test all conversion paths between supported formats (u8, u16, f32)
#[test]
fn all_conversion_paths() {
    // u8 -> u16 -> u8
    let u8_val: u8 = 200;
    let u16_val: u16 = u8_val.convert();
    let back_u8: u8 = u16_val.convert();
    assert_eq!(u8_val, back_u8, "u8->u16->u8 failed");

    // u8 -> f32 -> u8
    let f32_val: f32 = u8_val.convert();
    let back_u8_f: u8 = f32_val.convert();
    assert_eq!(u8_val, back_u8_f, "u8->f32->u8 failed");

    // u16 -> u8 (downscale)
    let u16_full: u16 = 65535;
    let u8_down: u8 = u16_full.convert();
    assert_eq!(u8_down, 255, "u16->u8 downscale failed");

    // u16 -> f32 -> u16
    let u16_val2: u16 = 32768;
    let f32_from_u16: f32 = u16_val2.convert();
    let back_u16: u16 = f32_from_u16.convert();
    assert!(
        (u16_val2 as i32 - back_u16 as i32).abs() <= 1,
        "u16->f32->u16 failed"
    );

    // f32 -> u8
    let f32_half: f32 = 0.5;
    let u8_from_f32: u8 = f32_half.convert();
    assert!((u8_from_f32 as i32 - 127).abs() <= 1, "f32->u8 failed");

    // f32 -> u16
    let u16_from_f32: u16 = f32_half.convert();
    assert!((u16_from_f32 as i32 - 32767).abs() <= 1, "f32->u16 failed");
}
