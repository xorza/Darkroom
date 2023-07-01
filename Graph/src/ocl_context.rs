use std::cell::RefCell;

use uuid::Uuid;

use crate::invoke::{InvokeArgs, Invoker};

pub struct OclContext {
    context: ocl::Context,
    current_queue: RefCell<Option<ocl::Queue>>,
}

impl OclContext {
    pub fn new() -> anyhow::Result<OclContext> {
        let platform = ocl::Platform::default();
        let device = ocl::Device::first(platform)?;
        let context = ocl::Context::builder()
            .platform(platform)
            .devices(device)
            .build()?;

        Ok(OclContext {
            context,
            current_queue: RefCell::new(None),
        })
    }

}

impl Invoker for OclContext {
    fn start(&self) {
        let queue = ocl::Queue::new(&self.context, self.context.devices()[0], None).unwrap();
        let mut current_queue = self.current_queue.borrow_mut();
        *current_queue = Some(queue);
    }

    fn call(&self, _function_id: Uuid, _context_id: Uuid, _inputs: &InvokeArgs, _outputs: &mut InvokeArgs) -> anyhow::Result<()> {
        let current_queue = self.current_queue.borrow();
        let _current_queue = current_queue.as_ref().unwrap();

        todo!("Call OpenCL function");
    }

    fn finish(&self) {
        let mut current_queue = self.current_queue.borrow_mut();
        *current_queue = None;
    }
}
