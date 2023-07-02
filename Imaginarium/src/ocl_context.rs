pub struct OclContext {
    context: ocl::Context,
    queue: ocl::Queue,
}

impl OclContext {
    pub fn new() -> anyhow::Result<OclContext> {
        let platform = ocl::Platform::default();
        let device = ocl::Device::first(platform)?;
        let context = ocl::Context::builder()
            .platform(platform)
            .devices(device)
            .build()?;

        let queue = ocl::Queue::new(&context, device, None).unwrap();

        Ok(OclContext {
            context,
            queue,
        })
    }
}
