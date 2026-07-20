use crate::math::vec2us::Vec2us;
use crate::stacking::registration::config::{InterpolationMethod, WarpParams};
use crate::stacking::registration::resample::kernel;
use glam::Vec2;
use imaginarium::Buffer2;

#[inline]
pub(crate) fn interpolate_lanczos(
    data: &Buffer2<f32>,
    pos: Vec2,
    a: usize,
    border_value: f32,
) -> f32 {
    match a {
        2 => interpolate_lanczos_impl::<2, 4>(data, pos, border_value),
        3 => interpolate_lanczos_impl::<3, 6>(data, pos, border_value),
        4 => interpolate_lanczos_impl::<4, 8>(data, pos, border_value),
        _ => panic!("Unsupported Lanczos parameter: {a}"),
    }
}

#[inline]
fn interpolate_lanczos_impl<const A: usize, const SIZE: usize>(
    data: &Buffer2<f32>,
    pos: Vec2,
    border_value: f32,
) -> f32 {
    let (x, y) = (pos.x, pos.y);
    let (w, h) = (data.width(), data.height());
    if !kernel::source_footprint_contains(pos, Vec2us::new(w, h)) {
        return border_value;
    }

    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let a_i32 = A as i32;
    let kx0 = x0 - a_i32 + 1;
    let ky0 = y0 - a_i32 + 1;
    if kx0 < 0 || ky0 < 0 || kx0 + SIZE as i32 > w as i32 || ky0 + SIZE as i32 > h as i32 {
        return kernel::bilinear_sample(data, pos, border_value);
    }

    let lut = kernel::get_lanczos_lut(A);

    let mut wx = [0.0f32; SIZE];
    let mut wy = [0.0f32; SIZE];
    for (i, weight) in wx.iter_mut().enumerate() {
        *weight = lut.lookup(fx - (i as i32 - a_i32 + 1) as f32);
    }
    for (j, weight) in wy.iter_mut().enumerate() {
        *weight = lut.lookup(fy - (j as i32 - a_i32 + 1) as f32);
    }

    let pixels = data.pixels();
    let mut sum = 0.0f32;
    for (j, &wyj) in wy.iter().enumerate() {
        let py = y0 - a_i32 + 1 + j as i32;
        let row_off = py as usize * w;
        for (i, &wxi) in wx.iter().enumerate() {
            let px = x0 - a_i32 + 1 + i as i32;
            sum += pixels[row_off + px as usize] * wxi * wyj;
        }
    }
    let total_weight = wx.iter().sum::<f32>() * wy.iter().sum::<f32>();
    if total_weight.abs() < 1e-10 {
        sum
    } else {
        sum / total_weight
    }
}

pub(crate) fn interpolate(data: &Buffer2<f32>, pos: Vec2, params: &WarpParams) -> f32 {
    match params.method {
        InterpolationMethod::Nearest => kernel::interpolate_nearest(data, pos, params.border_value),
        InterpolationMethod::Bilinear => kernel::bilinear_sample(data, pos, params.border_value),
        InterpolationMethod::Bicubic => kernel::interpolate_bicubic(data, pos, params.border_value),
        InterpolationMethod::Lanczos2 => interpolate_lanczos(data, pos, 2, params.border_value),
        InterpolationMethod::Lanczos3 => interpolate_lanczos(data, pos, 3, params.border_value),
        InterpolationMethod::Lanczos4 => interpolate_lanczos(data, pos, 4, params.border_value),
    }
}
