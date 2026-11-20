# Architecture

## 分层

```text
平台 crate（窗口/事件/生命周期）
  └─ 创建 wgpu::Surface<'static> + 驱动帧循环
       └─ renderer-core::Renderer
            └─ wgpu（唯一 GPU 后端）
```

- `renderer-core` 不依赖 winit、SDL、Android SDK 或 Web API，只依赖 wgpu、glam、gltf、image、encoding_rs、glyphon。
- 平台 crate 把原生窗口/canvas/ANativeWindow 转成 `wgpu::Surface<'static>`，交给同一个 `Renderer`。
- `renderer_core::model` 提供 OBJ / glTF / GLB / PMX 导入，统一输出索引化三角网格。
- `renderer_core::model::pmx` 解析 PMX 2.0/2.1 的几何、UV、材质、贴图路径、骨骼与权重；表情/刚体留后续版本。
- `renderer_core::animation::VmdClip` 解析 VMD 骨骼/表情轨道并采样为 `Pose`；`skinning` 根据骨骼层级计算全局矩阵。
- `renderer_core::overlay::FpsOverlay` 用 glyphon/cosmic-text 在 mesh pass 末尾绘制屏幕空间 FPS 文字。

## 关键类型

- `renderer_core::backend::create_instance_for(backends)`：按目标平台创建 `wgpu::Instance`。wgpu 30 的 `InstanceDescriptor` 不再实现 `Default`，必须用 `new_without_display_handle()`。
- `Renderer::new(instance, surface, size, backends)`：内置三角形快捷入口。
- `Renderer::new_with_mesh_and_textures(...)`：用导入的 `Mesh` 与 PMX 目录贴图创建管线与 GPU 缓冲。
- `model::Mesh`：位置/法线/颜色/UV/骨骼索引/权重 + u32 索引 + 材质分组 + 骨骼列表。
- `model::load_from_path(path)`：按扩展名加载 `.obj` / `.gltf` / `.glb` / `.pmx`。
- `Renderer::enable_fps_overlay(font_path)`：开启左上角 FPS 叠加；`None` 使用系统等宽字体。
- `Renderer::set_animation(clip)` / `set_pose(pose)`：绑定 VMD 或直接写入骨骼姿态。
- `Renderer::resize(size)`：零尺寸跳过；重建 surface config、深度纹理与文字 viewport。
- `Renderer::render_frame()`：采样动画、计算 MVP、写 uniform/骨骼矩阵，绘制 outline + toon，最后在同一个 pass 末尾画 FPS 文字。

## 着色器

`toon.wgsl` 与 `outline.wgsl` 是主渲染着色器。顶点包含 position/normal/color/uv/bone_indices/bone_weights，uniform 含 MVP 与 model 矩阵，read-only storage buffer 保存骨骼矩阵；片段阶段做 toon 渐变、贴图采样与 sphere map。FPS 文字由 glyphon 生成独立 pipeline，在同一个 render pass 末尾以 depth `Always` 绘制，不参与 3D 深度。WGSL 是 wgpu 全后端保证的语言；不引入 GLSL/SPIR-V 预编译步骤。

## 后端策略

- 桌面：`Backends::VULKAN` 写死，日志打印 `Backend: Vulkan`。
- Web：`Backends::BROWSER_WEBGPU`。
- Android：`Backends::VULKAN`。
- Apple：`Backends::METAL`（原生 Metal，不需要 MoltenVK）。

## 为什么不做自研 RHI

同时维护 Vulkan/Metal/DX12 三套语义（descriptor、同步、render pass、shader 交叉编译）等同于重造 wgpu。本项目把精力放在内核抽象和平台脚手架上，而不是重复实现后端。
