//! The operations on a stacked master, which plan nothing: each must give back its working set
//! before the next one starts.

use lumos::{Denoise, ExtractBackground, ImageDimensions, LinearImage, Stretch};

use crate::alone::alone;
use crate::session::Session;
use crate::star_field::StarField;

const SIDE: usize = 256;

/// A three-channel master through background extraction, denoising and a stretch keeps, after
/// each, exactly the heap it held before: its planes, and nothing an operation kept. On one
/// worker, as an exact count takes — see [`Session::on_workers`].
#[test]
fn each_operation_gives_back_its_working_set() {
    alone(|| {
        let session = Session::on_workers("lumos_budget_image_ops", 1);
        let field = StarField::new(SIDE, 40, 0x0B5);
        let channels = (0..3).map(|index| field.frame(index).channel(0).to_vec());
        let mut master =
            LinearImage::from_planar_channels(ImageDimensions::new((SIDE, SIDE), 3), channels);
        let mut apply = |label: &str, op: fn(&mut LinearImage)| {
            assert_eq!(session.measure(|| op(&mut master)).kept_bytes, 0, "{label}");
        };
        apply("background extraction", |master| {
            ExtractBackground::default().apply(master).unwrap();
        });
        apply("denoising", |master| {
            Denoise::default().apply(master).unwrap();
        });
        apply("stretch", |master| {
            Stretch::default().apply(master).unwrap();
        });
    });
}
