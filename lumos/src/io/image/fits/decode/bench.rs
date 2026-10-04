//! Benchmarks for the FITS decode.

use common::TempDir;
use fits_well::FitsWriter;
use fits_well::image::Image;
use quickbench::quick_bench;

use crate::internals::test_rng::TestRng;
use crate::io::image::fits::decode::load_linear_fits;
use crate::io::image::load_context::LoadContext;

/// A 24 MP 16-bit frame (`BITPIX = 16`, `BZERO = 32768`) of random samples, loaded from a file the
/// page cache already holds: the conversion, not the disk.
#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_fits_decode_u16(b: quickbench::Bencher) {
    let (width, height) = (6000, 4000);
    let mut rng = TestRng::new(5);
    let samples: Vec<u16> = (0..width * height)
        .map(|_| u16::try_from(rng.next_u64() >> 48).unwrap())
        .collect();
    let image = Image::from_u16(vec![width, height], &samples).unwrap();
    let dir = TempDir::new("lumos-fits-bench");
    let path = dir.join("frame.fits");
    let mut writer = FitsWriter::new(std::fs::File::create(&path).unwrap());
    writer.write_image(&image, None).unwrap();
    writer.into_inner().sync_all().unwrap();
    b.bench(|| load_linear_fits(&path, &LoadContext::default()).unwrap());
}
