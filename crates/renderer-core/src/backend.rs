use wgpu::{Backends, Instance, InstanceDescriptor};

/// 桌面端强制使用的 GPU 后端。
pub const BACKENDS: Backends = Backends::VULKAN;

/// 后端名称，用于日志与诊断。
pub const BACKEND_LABEL: &str = "Vulkan";

/// 创建限定为 Vulkan 的 wgpu Instance。
///
/// wgpu 30 的 `InstanceDescriptor` 不再实现 `Default`，
/// 必须用 `new_without_display_handle()` 构造。
pub fn create_instance() -> Instance {
    Instance::new(InstanceDescriptor {
        backends: BACKENDS,
        ..InstanceDescriptor::new_without_display_handle()
    })
}
