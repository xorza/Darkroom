use bytemuck::{Pod, Zeroable};
use pollster::FutureExt;

pub(crate) struct WgpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

fn aligned_size_of_uniform<U: Sized>() -> wgpu::BufferAddress {
    let uniform_size = std::mem::size_of::<U>();
    let uniform_align = 256;
    let uniform_padded_size = (uniform_size + uniform_align - 1) / uniform_align * uniform_align;

    uniform_padded_size as wgpu::BufferAddress
}

impl WgpuContext {
    pub fn new() -> anyhow::Result<WgpuContext> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            dx12_shader_compiler: wgpu::Dx12Compiler::Dxc { dxil_path: None, dxc_path: None },
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .block_on()
            .expect("Unable to find a suitable GPU adapter.");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .block_on()
            .expect("Unable to find a suitable GPU device.");

        Ok(WgpuContext { device, queue })
    }
}


#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(crate) struct Vert(pub [f32; 2], pub [f32; 2]);

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(crate) struct Rect(pub [Vert; 4]);

impl Rect {
    pub fn one() -> Rect {
        Rect([
            Vert([0.0, 0.0], [0.0, 0.0]),
            Vert([1.0, 0.0], [1.0, 0.0]),
            Vert([0.0, 1.0], [0.0, 1.0]),
            Vert([1.0, 1.0], [1.0, 1.0]),
        ])
    }

    pub fn new(width: f32, height: f32, tex_width: f32, tex_height: f32) -> Rect {
        Rect([
            Vert([0.0, 0.0], [0.0, 0.0]),
            Vert([width, 0.0], [tex_width, 0.0]),
            Vert([0.0, height], [0.0, tex_height]),
            Vert([width, height], [tex_width, tex_height]),
        ])
    }

    pub fn desc<'a>() -> wgpu::VertexBufferLayout<'a> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vert>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    format: wgpu::VertexFormat::Float32x2,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                    format: wgpu::VertexFormat::Float32x2,
                    shader_location: 1,
                },
            ],
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.0)
    }
    pub fn size_in_bytes(&self) -> u32 {
        std::mem::size_of::<Rect>() as u32
    }
    pub fn vert_count(&self) -> u32 {
        self.0.len() as u32
    }
    pub fn stride(&self) -> u32 {
        std::mem::size_of::<Vert>() as u32
    }
}
