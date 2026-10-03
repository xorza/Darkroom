// Shared by the shaders over tightly packed buffers: how a channel value of
// `elem_size` bytes (1 = u8, 2 = u16, 4 = f32) sits in a little-endian u32
// word, the one rounding rule for narrowing into an integer type, and the
// linear index of a 2-D dispatch.

const NO_ALPHA: u32 = 0xffffffffu;
const WORKGROUP_SIZE: u32 = 256u;

// The value that stands for one: 255, 65535, or 1 for f32.
fn full_scale(elem_size: u32) -> f32 {
    switch elem_size {
        case 1u: { return 255.0; }
        case 2u: { return 65535.0; }
        default: { return 1.0; }
    }
}

// The element `shift` bits into `word`, in its own units.
fn unpack(word: u32, shift: u32, elem_size: u32) -> f32 {
    switch elem_size {
        case 1u: { return f32((word >> shift) & 0xFFu); }
        case 2u: { return f32((word >> shift) & 0xFFFFu); }
        default: { return bitcast<f32>(word); }
    }
}

// A value in an element's own units, narrowed as the CPU narrows it: rounded
// to nearest with ties to even (WGSL `round`), saturated to the type's range,
// and placed `shift` bits into a word. f32 is stored as it is.
fn pack(value: f32, shift: u32, elem_size: u32) -> u32 {
    if elem_size == 4u {
        return bitcast<u32>(value);
    }
    return u32(round(clamp(value, 0.0, full_scale(elem_size)))) << shift;
}

// The invocation's index in a dispatch laid out as rows of `groups.x`
// workgroups, each `WORKGROUP_SIZE` wide.
fn linear_index(gid: vec3<u32>, groups: vec3<u32>) -> u32 {
    return gid.y * groups.x * WORKGROUP_SIZE + gid.x;
}
