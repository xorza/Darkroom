// GPU Sigma Clipping Compute Shader
//
// Each thread processes one pixel position across all frames in the stack.
// Uses mean-based iterative sigma clipping (not median, for GPU efficiency).
//
// Algorithm:
// 1. Load all N values for this pixel from the frame stack
// 2. Compute mean and standard deviation
// 3. Mark values beyond sigma threshold as invalid
// 4. Repeat for max_iterations or until no values are clipped
// 5. Output mean of remaining valid values

// Maximum number of frames supported per dispatch
// Limited by WGSL array size and register pressure
const MAX_FRAMES: u32 = 128u;

struct Params {
    width: u32,
    height: u32,
    frame_count: u32,
    sigma: f32,
    max_iterations: u32,
    // Padding to align to 16 bytes (uniform buffers require 16-byte alignment)
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> frames: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    // Bounds check
    if x >= params.width || y >= params.height {
        return;
    }

    let pixel_idx = y * params.width + x;
    let pixels_per_frame = params.width * params.height;

    // Ensure we don't exceed maximum supported frames
    let frame_count = min(params.frame_count, MAX_FRAMES);

    // Load all values for this pixel position into local arrays
    var values: array<f32, MAX_FRAMES>;
    var valid: array<bool, MAX_FRAMES>;

    for (var i = 0u; i < frame_count; i++) {
        values[i] = frames[i * pixels_per_frame + pixel_idx];
        valid[i] = true;
    }

    var valid_count = frame_count;

    // Iterative sigma clipping
    for (var iter = 0u; iter < params.max_iterations; iter++) {
        // Need at least 2 values for meaningful statistics
        if valid_count <= 2u {
            break;
        }

        // Compute mean of valid values
        var sum = 0.0;
        var n = 0u;
        for (var i = 0u; i < frame_count; i++) {
            if valid[i] {
                sum += values[i];
                n++;
            }
        }
        let mean = sum / f32(n);

        // Compute variance (sum of squared differences from mean)
        var sum_sq = 0.0;
        for (var i = 0u; i < frame_count; i++) {
            if valid[i] {
                let diff = values[i] - mean;
                sum_sq += diff * diff;
            }
        }
        let variance = sum_sq / f32(n);
        let std_dev = sqrt(variance);

        // If standard deviation is essentially zero, all values are identical
        if std_dev < 1e-10 {
            break;
        }

        // Mark outliers as invalid
        let threshold = params.sigma * std_dev;
        var clipped = 0u;
        for (var i = 0u; i < frame_count; i++) {
            if valid[i] && abs(values[i] - mean) > threshold {
                valid[i] = false;
                clipped++;
            }
        }

        // If no values were clipped, we've converged
        if clipped == 0u {
            break;
        }
        valid_count -= clipped;
    }

    // Compute final mean of remaining valid values
    var final_sum = 0.0;
    var final_n = 0u;
    for (var i = 0u; i < frame_count; i++) {
        if valid[i] {
            final_sum += values[i];
            final_n++;
        }
    }

    // Handle edge case where all values were clipped (shouldn't happen with proper sigma)
    if final_n == 0u {
        output[pixel_idx] = 0.0;
    } else {
        output[pixel_idx] = final_sum / f32(final_n);
    }
}
