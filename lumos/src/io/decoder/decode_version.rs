//! [`DECODE_VERSION`]: the version of every decode a frame cache can hold, derived from the
//! references the characterization tests compare those decodes with.

use crate::io::decoder::fnv1a;

/// The decode version every cache key carries: a hash of the reference of each decode a frame
/// cache can hold, and of the statistics it keeps beside the planes — see
/// `internals::characterization`. A decode that moves past the tolerance fails its test until its
/// reference is written anew, and that moves this version, so a kept cache written by an older
/// decoder is never reused. One that moves within the tolerance keeps the version, as its values
/// are the reference's to that tolerance. Not bumped by hand.
pub(crate) const DECODE_VERSION: u64 = fnv1a::fingerprint(&[
    include_bytes!("../../../snapshots/fits_linear.ron"),
    include_bytes!("../../../snapshots/float_tiff.ron"),
    include_bytes!("../../../snapshots/fits_cfa.ron"),
    include_bytes!("../../../snapshots/raw_cfa.ron"),
    include_bytes!("../../../snapshots/demosaic.ron"),
    include_bytes!("../../../snapshots/frame_stats.ron"),
]);
