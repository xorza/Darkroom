use wgpu::{Adapter, Device, Queue, SurfaceConfiguration, TextureView};

use crate::app_base::App;
use crate::event::{Event, EventResult};

struct UiAppInternal {}

impl App for UiAppInternal {
    fn init(
        _config: &SurfaceConfiguration,
        _adapter: &Adapter,
        _device: &Device,
        _queue: &Queue)
        -> Self {
        todo!()
    }

    fn update(
        &mut self,
        _event: Event)
        -> EventResult {
        todo!()
    }

    fn render(
        &mut self,
        _view: &TextureView,
        _device: &Device,
        _queue: &Queue) {
        todo!()
    }
}
