use std::time::Instant;

use glam::UVec2;
use pollster::FutureExt;
use wgpu::{Dx12Compiler, Features, RequestAdapterOptions};
use winit::{
    event::{self, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
};

use crate::event::{Event, EventResult};

pub trait App: 'static + Sized {
    fn init(
        config: &wgpu::SurfaceConfiguration,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Self;
    fn resize(&mut self, window_size: UVec2);
    fn update(&mut self, event: Event) -> EventResult;
    fn render(
        &mut self,
        view: &wgpu::TextureView,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    );
}

struct Setup {
    window: winit::window::Window,
    event_loop: EventLoop<()>,
    instance: wgpu::Instance,
    size: winit::dpi::PhysicalSize<u32>,
    surface: wgpu::Surface,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn setup(title: &str) -> Setup {
    let event_loop = EventLoop::new();
    let window =
        winit::window::WindowBuilder::new()
            .with_title(title)
            .build(&event_loop)
            .expect("Failed to create window.");

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        dx12_shader_compiler: Dx12Compiler::Dxc { dxil_path: None, dxc_path: None },
    });
    let size = window.inner_size();
    let surface = unsafe {
        instance.create_surface(&window).unwrap()
    };

    let adapter = instance
        .request_adapter(&RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        })
        .block_on()
        .expect("No suitable GPU adapters found on the system.");

    // let adapter_info = adapter.get_info();
    // println!("Using {} ({:?})", adapter_info.name, adapter_info.backend);

    // Make sure we use the texture resolution limits from the adapter, so we can support images the size of the surface.
    let limits = adapter.limits().using_resolution(adapter.limits());

    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                label: None,
                features: Features::empty(),
                limits,
            },
            None,
        )
        .block_on()
        .expect("Unable to find a suitable GPU adapter.");

    Setup {
        window,
        event_loop,
        instance,
        size,
        surface,
        adapter,
        device,
        queue,
    }
}

fn start<E: App>(
    Setup {
        window,
        event_loop,
        instance,
        size,
        surface,
        adapter,
        device,
        queue,
    }: Setup,
) {
    let mut config = surface
        .get_default_config(&adapter, size.width, size.height)
        .expect("Surface isn't supported by the adapter.");
    let surface_view_format = config.format.add_srgb_suffix();
    config.view_formats.push(surface_view_format);
    surface.configure(&device, &config);

    let mut app = E::init(&config, &adapter, &device, &queue);

    let mut last_frame_inst = Instant::now();
    let (mut frame_count, mut accum_time) = (0, 0.0);

    event_loop.run(move |event, _target, control_flow| {
        let _ = (&instance, &adapter); // force ownership by the closure
        let mut result: EventResult = EventResult::Continue;

        match event {
            event::Event::RedrawEventsCleared => {
                if let Some(error) = device.pop_error_scope().block_on() {
                    panic!("Device error: {:?}", error);
                }

                window.request_redraw();
            }
            event::Event::WindowEvent {
                event:
                WindowEvent::Resized(size)
                | WindowEvent::ScaleFactorChanged {
                    new_inner_size: &mut size,
                    ..
                },
                ..
            } => {
                config.width = size.width.max(1);
                config.height = size.height.max(1);

                app.resize(UVec2::new(config.width, config.height));

                surface.configure(&device, &config);
            }

            event::Event::RedrawRequested(_) => {
                {
                    accum_time += last_frame_inst.elapsed().as_secs_f32();
                    last_frame_inst = Instant::now();
                    frame_count += 1;
                    if frame_count == 100 {
                        println!(
                            "Avg frame time {}ms",
                            accum_time * 1000.0 / frame_count as f32
                        );
                        accum_time = 0.0;
                        frame_count = 0;
                    }
                }

                let frame = match surface.get_current_texture() {
                    Ok(frame) => frame,
                    Err(_) => {
                        surface.configure(&device, &config);
                        surface
                            .get_current_texture()
                            .expect("Failed to acquire next surface texture.")
                    }
                };
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(surface_view_format),
                    ..wgpu::TextureViewDescriptor::default()
                });

                app.render(&view, &device, &queue);

                frame.present();
            }

            _ => {
                let event = Event::from(event);
                if event != Event::Unknown {
                    result = app.update(event);
                }
            }
        }

        if result == EventResult::Exit {
            *control_flow = ControlFlow::Exit;
        }
    });
}

pub fn run<E: App>(title: &str) {
    let setup = setup(title);
    start::<E>(setup);
}
