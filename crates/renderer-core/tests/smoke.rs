use renderer_core::backend;

#[test]
fn vulkan_adapter_and_device_are_available() {
    let instance = backend::create_instance();
    let adapter_count = pollster::block_on(instance.enumerate_adapters(backend::BACKENDS)).len();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        ..Default::default()
    }))
    .unwrap_or_else(|error| {
        panic!("Vulkan adapter request failed (detected {adapter_count}): {error:?}")
    });

    let (_device, _queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("failed to request device");
}
