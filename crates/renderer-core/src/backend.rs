use wgpu::{Backends, Instance, InstanceDescriptor};

/// 桌面端强制使用的 GPU 后端。
pub const BACKENDS: Backends = Backends::VULKAN;

/// 桌面端后端名称，用于日志与诊断。
pub const BACKEND_LABEL: &str = "Vulkan";

/// 创建限定为指定后端的 wgpu Instance。
///
/// wgpu 30 的 `InstanceDescriptor` 不再实现 `Default`，
/// 必须用 `new_without_display_handle()` 构造。
pub fn create_instance_for(backends: Backends) -> Instance {
    Instance::new(InstanceDescriptor {
        backends,
        ..InstanceDescriptor::new_without_display_handle()
    })
}

/// 创建桌面端 Vulkan Instance。
pub fn create_instance() -> Instance {
    create_instance_for(BACKENDS)
}

/// 后端显示名称。
pub fn label(backends: Backends) -> &'static str {
    if backends.contains(Backends::VULKAN) {
        "Vulkan"
    } else if backends.contains(Backends::METAL) {
        "Metal"
    } else if backends.contains(Backends::DX12) {
        "DirectX 12"
    } else if backends.contains(Backends::BROWSER_WEBGPU) {
        "WebGPU"
    } else if backends.contains(Backends::GL) {
        "OpenGL"
    } else {
        "Unknown"
    }
}
