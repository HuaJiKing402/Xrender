# Architecture

## 分层

```text
平台 crate（窗口/事件/生命周期）
  └─ 创建 wgpu::Surface<'static> + 驱动帧循环
       └─ renderer-core::Renderer
            └─ wgpu（唯一 GPU 后端）
```

- `renderer-core` 不依赖 winit、SDL、Android SDK 或 Web API，只依赖 wgpu、glam、gltf。
- 平台 crate 把原生窗口/canvas/ANativeWindow 转成 `wgpu::Surface<'static>`，交给同一个 `Renderer`。
- `renderer_core::model` 提供 OBJ / glTF / GLB / PMX 导入，统一输出索引化三角网格。
- `renderer_core::model::pmx` 是 PMX 2.0/2.1 基础解析器，当前读取几何与材质颜色；骨骼/表情/物理见 MMD 路线图。

## 关键类型

- `renderer_core::backend::create_instance_for(backends)`：按目标平台创建 `wgpu::Instance`。wgpu 30 的 `InstanceDescriptor` 不再实现 `Default`，必须用 `new_without_display_handle()`。
- `Renderer::new(instance, surface, size, backends)`：内置三角形快捷入口。
- `Renderer::new_with_mesh(instance, surface, size, backends, mesh)`：用导入的 `Mesh` 创建管线与 GPU 缓冲。
- `model::Mesh`：位置/法线/颜色 + u32 索引；`normalize()` 自动居中缩放；`ensure_normals()` 补面法线。
- `model::load_from_path(path)`：按扩展名加载 `.obj` / `.gltf` / `.glb` / `.pmx`。
- `Renderer::resize(size)`：零尺寸跳过；重建 surface config，不重建 pipeline。
- `Renderer::render_frame()`：用 glam 计算 MVP 矩阵写入 uniform，处理 `CurrentSurfaceTexture` 状态后绘制索引网格并呈现。

## 着色器

`crates/renderer-core/src/shaders/triangles.wgsl` 是唯一的着色器源码。顶点包含 position/normal/color，uniform 含 MVP 与 model 矩阵，片段阶段做简单方向光。WGSL 是 wgpu 全后端保证的语言；不引入 GLSL/SPIR-V 预编译步骤。

## 后端策略

- 桌面：`Backends::VULKAN` 写死，日志打印 `Backend: Vulkan`。
- Web：`Backends::BROWSER_WEBGPU`。
- Android：`Backends::VULKAN`。
- Apple：`Backends::METAL`（原生 Metal，不需要 MoltenVK）。

## 为什么不做自研 RHI

同时维护 Vulkan/Metal/DX12 三套语义（descriptor、同步、render pass、shader 交叉编译）等同于重造 wgpu。本项目把精力放在内核抽象和平台脚手架上，而不是重复实现后端。
