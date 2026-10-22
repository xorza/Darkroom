//! Natural cubic spline numerics: tridiagonal solve for second derivatives plus per-interval
//! evaluation, used to interpolate the tile-grid sky/sigma values C²-continuously.

pub(crate) mod natural_spline;
pub(crate) mod spline_segment;

#[cfg(test)]
mod tests;
