// GPU Threshold Mask Creation Compute Shader
//
// Creates a binary mask of pixels above detection threshold.
// Each thread processes one pixel.
//
// Formula: mask[i] = pixels[i] > background[i] + sigma * noise[i]

struct Params {
    width: u32,
    height: u32,
    sigma_threshold: f32,
    // Padding to align to 16 bytes
    _pad0: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> pixels: array<f32>;
@group(0) @binding(2) var<storage, read> background: array<f32>;
@group(0) @binding(3) var<storage, read> noise: array<f32>;
@group(0) @binding(4) var<storage, read_write> mask: array<atomic<u32>>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    // Bounds check
    if x >= params.width || y >= params.height {
        return;
    }

    let idx = y * params.width + x;

    // Compute threshold: background + sigma * noise
    let bg = background[idx];
    let n = max(noise[idx], 1e-6); // Prevent division issues
    let threshold = bg + params.sigma_threshold * n;

    // Check if pixel is above threshold
    let above_threshold = pixels[idx] > threshold;

    // Pack result into bitmask
    // Each u32 holds 32 mask bits (one per pixel in x direction)
    let bit_position = x % 32u;
    let mask_idx = (y * ((params.width + 31u) / 32u)) + (x / 32u);

    // Use atomicOr to set the bit (multiple threads may write to same u32)
    if above_threshold {
        atomicOr(&mask[mask_idx], 1u << bit_position);
    }
}
