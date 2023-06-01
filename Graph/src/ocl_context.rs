use std::cell::RefCell;
use uuid::Uuid;
use crate::invoke::{Args, Invoker};

pub struct OclContext {
    context: ocl::Context,
    current_queue: RefCell<Option<ocl::Queue>>,
}

impl OclContext {
    pub fn new() -> OclContext {
        let platform = ocl::Platform::default();
        let device = ocl::Device::first(platform).unwrap();
        let context = ocl::Context::builder()
            .platform(platform)
            .devices(device.clone())
            .build()
            .unwrap();

        OclContext {
            context,
            current_queue: RefCell::new(None),
        }
    }

    pub fn start_queue(&mut self) {}
}

impl Invoker for OclContext {
    fn start(&self) {
        let queue = ocl::Queue::new(&self.context, self.context.devices()[0], None).unwrap();
        let mut current_queue = self.current_queue.borrow_mut();
        *current_queue = Some(queue.clone());
    }

    fn call(&self, _function_name: &str, _context_id: Uuid, _inputs: &Args, _outputs: &mut Args) {
        let current_queue = self.current_queue.borrow();
        let _current_queue = current_queue.as_ref().unwrap();

        // todo!("Call OpenCL function");
    }

    fn finish(&self) {
        let mut current_queue = self.current_queue.borrow_mut();
        *current_queue = None;
    }
}
