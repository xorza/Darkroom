use std::rc::Rc;

use glam::UVec2;
use wgpu::{Adapter, Device, Queue, SurfaceConfiguration, Texture, TextureView};

use crate::app_base::{App, InitInfo, RenderInfo};
use crate::canvas::Canvas;
use crate::event::{Event, EventResult};
use crate::renderer::{Renderer, WgpuRenderer};
use crate::view::View;

pub struct UiApp {
    window_size: UVec2,
    renderer: WgpuRenderer,
    view: Rc<dyn View>,
    id_texture: Option<Texture>,
}

impl App for UiApp {
    fn init(init: InitInfo) -> Self {
        let window_size = UVec2::new(init.surface_config.width, init.surface_config.height);

        Self {
            window_size,
            renderer: WgpuRenderer::new(init),
            view: Rc::new(Canvas::new()),
            id_texture: None,
        }
    }

    fn update(&mut self, event: Event) -> EventResult {
        match event {
            Event::WindowClose => EventResult::Exit,
            Event::Resize(_size) => EventResult::Redraw,

            _ => EventResult::Continue
        }
    }

    fn render(&self, render_info: RenderInfo) {
        self.renderer.render_view(render_info, self.window_size, self.view.as_ref());
    }

    fn resize(&mut self, device: &Device,_queue: &wgpu::Queue,window_size: UVec2) {
        self.window_size = window_size;

        let mut should_recreate: bool = false;
        if self.id_texture.is_none() {
            should_recreate = true;
        } else {
            let texture = self.id_texture.as_ref().unwrap();
            if texture.width() != self.window_size.x || texture.height() != self.window_size.y {
                should_recreate = true;
            }
        }

        if should_recreate {
            let id_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Id Texture"),
                size: wgpu::Extent3d {
                    width: self.window_size.x,
                    height: self.window_size.y,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R32Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });

            self.id_texture = Some(id_texture);
        }
    }
}

impl UiApp {
    pub fn window_size(&self) -> UVec2 {
        self.window_size
    }
}