use pollster::FutureExt;

pub struct WgpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
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
