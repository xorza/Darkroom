#[cfg(test)]
mod graph_tests {
    use ocl::{Buffer, Context, Device, Platform, Queue};

    #[test]
    fn it_works() {
        let vec1 = vec![1.0, 2.0, -4.0, 4.0, 5.0];
        let vec2 = vec![5.0, 4.0, 10.0, 2.0, 1.0];
        let mut vec3 = vec![0.0; 5];

        let platform = Platform::default();
        let device = Device::first(platform).unwrap();
        let context = Context::builder()
            .platform(platform)
            .devices(device.clone())
            .build()
            .unwrap();
        let queue = Queue::new(&context, device, None).unwrap();
        let program = ocl::Program::builder()
            .devices(device)
            .src("
            __kernel void add(__global const float* vec1, __global const float* vec2, __global float* vec3) {
                int gid = get_global_id(0);
                vec3[gid] = vec1[gid] + vec2[gid];
            }
        ")
            .build(&context)
            .unwrap();

        let buffer_vec1 = Buffer::<f32>::builder()
            .queue(queue.clone())
            .len(vec1.len())
            .copy_host_slice(&vec1)
            .build()
            .unwrap();
        let buffer_vec2 = Buffer::<f32>::builder()
            .queue(queue.clone())
            .len(vec2.len())
            .copy_host_slice(&vec2)
            .build()
            .unwrap();
        let buffer_vec3 = Buffer::<f32>::builder()
            .queue(queue.clone())
            .len(vec3.len())
            .build()
            .unwrap();

        let kernel = ocl::Kernel::builder()
            .name("add")
            .program(&program)
            .queue(queue.clone())
            .global_work_size(vec1.len())
            .arg(&buffer_vec1)
            .arg(&buffer_vec2)
            .arg(&buffer_vec3)
            .build()
            .unwrap();

        unsafe {
            kernel.enq().unwrap();
        }

        buffer_vec3.read(&mut vec3).enq().unwrap();

        for (i, v) in vec3.iter().cloned().enumerate() {
            assert_eq!(v, 6.0, "The value of vec3[{}] is not equal to 6.0", i);
        }
    }
}
