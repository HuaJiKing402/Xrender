# Architecture

## 分层

```text
平台 crate（窗口/事件/生命周期）
  └─ 创建 wgpu::Surface<'static> + 驱动帧循环
       └─ renderer-core::Renderer
            └─ wgpu（唯一 GPU 后端）
```

- `renderer-core` 不依赖 winit、SDL、Android SDK 或 Web API，只依赖 wgpu。
- 平台 crate 把原生窗口/canvas/ANativeWindow 转成 `wgpu::Surface<'static>`，交给同一个 `Renderer`。

## 关键类型

- `renderer_core::backend::create_instance_for(backends)`：按目标平台创建 `wgpu::Instance`。wgpu 30 的 `InstanceDescriptor` 不再实现 `Default`，必须用 `new_without_display_handle()`。
- `Renderer::new(instance, surface, size, backends)`：请求 adapter/device、配置 surface、创建 WGSL 管线与顶点缓冲。
- `Renderer::resize(size)`：零尺寸跳过；重建 surface config，不重建 pipeline。
- `Renderer::render_frame()`：处理 `CurrentSurfaceTexture` 的 Success/Suboptimal/Timeout/Occluded/Outdated/Lost/Validation；录制 render pass 后用 `queue.present()` 呈现。

## 着色器

`crates/renderer-core/src/shaders/triangle.wgsl` 是唯一的着色器源码。WGSL 是 wgpu 全后端保证的语言；不引入 GLSL/SPIR-V 预编译步骤。

## 后端策略

- 桌面：`Backends::VULKAN` 写死，日志打印 `Backend: Vulkan`。
- Web：`Backends::BROWSER_WEBGPU`。
- Android：`Backends::VULKAN`。
- Apple：`Backends::METAL`（原生 Metal，不需要 MoltenVK）。

## 为什么不做自研 RHI

同时维护 Vulkan/Metal/DX12 三套语义（descriptor、同步、render pass、shader 交叉编译）等同于重造 wgpu。本项目把精力放在内核抽象和平台脚手架上，而不是重复实现后端。
