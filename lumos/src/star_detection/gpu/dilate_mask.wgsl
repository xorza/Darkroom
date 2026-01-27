// GPU Mask Dilation Compute Shader
//
// Dilates a binary mask by the specified radius.
// Each output pixel is 1 if any pixel within radius R is 1 in the input.
//
// Uses shared memory tiling for efficient neighbor access.

struct Params {
    width: u32,
    height: u32,
    radius: u32,
    // Width of packed mask in u32 units
    mask_width_u32: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> mask_in: array<u32>;
@group(0) @binding(2) var<storage, read_write> mask_out: array<atomic<u32>>;

// Shared memory for tile + halo
// Max radius supported is 8, so max tile is 16 + 2*8 = 32
// We'll use 32x32 shared memory
const TILE_SIZE: u32 = 16u;
const MAX_HALO: u32 = 8u;
const SHARED_SIZE: u32 = TILE_SIZE + 2u * MAX_HALO; // 32

var<workgroup> shared_tile: array<u32, 1024>; // 32x32 bits stored as bool approximation

// Helper to read a single bit from the packed mask
fn read_mask_bit(x: u32, y: u32) -> bool {
    if x >= params.width || y >= params.height {
        return false;
    }
    let bit_position = x % 32u;
    let mask_idx = y * params.mask_width_u32 + (x / 32u);
    return (mask_in[mask_idx] & (1u << bit_position)) != 0u;
}

@compute @workgroup_size(16, 16, 1)
fn main(
    @builtin(global_invocation_id) global_id: vec3<u32>,
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) workgroup_id: vec3<u32>
) {
    let radius = min(params.radius, MAX_HALO);

    // Global position of tile origin
    let tile_origin_x = workgroup_id.x * TILE_SIZE;
    let tile_origin_y = workgroup_id.y * TILE_SIZE;

    // Load tile + halo into shared memory
    // Each thread loads multiple elements to cover the halo
    let shared_width = TILE_SIZE + 2u * radius;
    let elements_per_thread = ((shared_width * shared_width) + 255u) / 256u;

    let tid = local_id.y * TILE_SIZE + local_id.x;

    for (var i = 0u; i < elements_per_thread; i++) {
        let elem_idx = tid + i * 256u;
        if elem_idx < shared_width * shared_width {
            let sy = elem_idx / shared_width;
            let sx = elem_idx % shared_width;

            // Global coordinates (may be outside image bounds)
            let gx_signed = i32(tile_origin_x) + i32(sx) - i32(radius);
            let gy_signed = i32(tile_origin_y) + i32(sy) - i32(radius);

            var val = false;
            if gx_signed >= 0 && gy_signed >= 0 {
                let gx = u32(gx_signed);
                let gy = u32(gy_signed);
                val = read_mask_bit(gx, gy);
            }

            // Store as 1 or 0 in shared memory
            shared_tile[sy * SHARED_SIZE + sx] = select(0u, 1u, val);
        }
    }

    workgroupBarrier();

    // Now each thread processes one output pixel
    let x = global_id.x;
    let y = global_id.y;

    if x >= params.width || y >= params.height {
        return;
    }

    // Position in shared memory (offset by radius for halo)
    let shared_x = local_id.x + radius;
    let shared_y = local_id.y + radius;

    // Check if any neighbor within radius is set
    var any_set = false;
    for (var dy = 0u; dy <= 2u * radius; dy++) {
        for (var dx = 0u; dx <= 2u * radius; dx++) {
            let sx = shared_x - radius + dx;
            let sy = shared_y - radius + dy;
            if shared_tile[sy * SHARED_SIZE + sx] != 0u {
                any_set = true;
                break;
            }
        }
        if any_set {
            break;
        }
    }

    // Write result to output mask
    let bit_position = x % 32u;
    let mask_idx = y * params.mask_width_u32 + (x / 32u);

    if any_set {
        atomicOr(&mask_out[mask_idx], 1u << bit_position);
    }
}
