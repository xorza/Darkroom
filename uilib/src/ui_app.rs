use std::rc::Rc;

use glam::UVec2;
use wgpu::{Adapter, Device, Queue, SurfaceConfiguration, TextureView};

use crate::app_base::{App, InitInfo, RenderInfo};
use crate::canvas::Canvas;
use crate::event::{Event, EventResult};
use crate::renderer::{Renderer, WgpuRenderer};
use crate::view::View;

pub struct UiApp {
    window_size: UVec2,
    renderer: WgpuRenderer,
    view: Rc<dyn View>,
}

impl App for UiApp {
    fn init(init: InitInfo) -> Self {
        Self {
            window_size: UVec2::new(init.surface_config.width, init.surface_config.height),
            renderer: WgpuRenderer::new(init),
            view: Rc::new(Canvas::new()),
        }
    }

    fn update(&mut self, event: Event) -> EventResult {
        match event {
            Event::WindowClose => EventResult::Exit,
            Event::Resize(size) => {
                self.window_size = size;

                EventResult::Redraw
            }

            _ => EventResult::Continue
        }
    }

    fn render(&self, render: RenderInfo) {
        self.renderer.render_view(render,self.window_size, self.view.as_ref());

    }
}

impl UiApp {
    pub fn window_size(&self) -> UVec2 {
        self.window_size
    }
}