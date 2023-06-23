use std::rc::Rc;

use glam::UVec2;
use wgpu::{Adapter, Device, Queue, SurfaceConfiguration, Texture, TextureView};

use crate::app_base::{App, RenderInfo};
use crate::canvas::Canvas;
use crate::event::{Event, EventResult};
use crate::renderer::Renderer;
use crate::view::View;

pub struct UiApp {
    window_size: UVec2,
    renderer: Renderer,
    view: Rc<dyn View>,
}

impl App for UiApp {
    fn init(device: &Device,
            queue: &Queue,
            surface_config: &SurfaceConfiguration) -> Self {
        let window_size = UVec2::new(surface_config.width, surface_config.height);
        let renderer = Renderer::new(device, queue, surface_config, window_size);

        let mut result = Self {
            window_size: UVec2::new(0, 0),
            renderer,
            view: Rc::new(Canvas::new()),
        };
        result.resize(device, queue, window_size);

        result
    }

    fn update(&mut self, event: Event) -> EventResult {
        match event {
            Event::WindowClose => EventResult::Exit,
            Event::Resize(_size) => EventResult::Redraw,

            _ => EventResult::Continue
        }
    }

    fn render(&self, render_info: RenderInfo) {
        self.renderer.render_view(&render_info);
    }

    fn resize(&mut self, device: &Device, queue: &Queue, window_size: UVec2) {
        self.window_size = window_size;
        self.renderer.resize(device, queue, window_size);
    }
}

impl UiApp {
    pub fn window_size(&self) -> UVec2 {
        self.window_size
    }
}