# CrossRender

一个用 Rust 写的多平台绘制引擎内核。首版以 `wgpu` 为唯一 GPU 后端，`winit` 处理窗口与事件，着色器统一用 WGSL；Windows 桌面强制 Vulkan。

## 当前状态

- `renderer-core`：平台无关渲染内核，负责 Instance/Adapter/Device、交换链配置、WGSL 网格管线、模型导入与帧呈现。
- `app-desktop`：Windows/Linux 桌面入口，已在本机验证 PMX 贴图材质、Toon/描边、骨骼蒙皮、VMD 动画与左上角 FPS 叠加。
- `app-web` / `app-android` / `app-apple`：Web、Android、Apple 平台脚手架（代码与配置齐全，未在本机验证）。
- `models/`：示例模型 `cube.obj`、`triangle.gltf`、`triangle.pmx`；大模型与 VMD 夹具仅本地回归，不入库。

## 快速开始（Windows 桌面）

```bash
MAX_FRAMES=5 cargo run -p app-desktop
```

预期输出：

```text
Model: builtin-triangle (3 vertices, 1 triangles, 1 materials, 0 textures, 0 bones)
Overlay: FPS
Backend: Vulkan
Adapter: AMD Radeon RX 5700 XT
Window: 800x600
rendered 5 frames
```

窗口中出现一个粉红/渐变三角形。按 Esc 或关闭窗口退出；设置 `MAX_FRAMES`（旧变量 `TRIANGLE_MAX_FRAMES` 仍兼容）后自动退出，便于 CI/脚本验证。

## 相机操作

- 左键拖拽：绕模型旋转
- 右键 / 中键拖拽：平移视口
- 滚轮：拉近 / 推远
- `R`：复位相机
- `Esc`：退出

当前不再自动旋转，模型展示完全由相机控制。

## FPS 叠加

桌面端左上角默认显示 `FPS: 60`，白字黑描边。字体由 `glyphon` + `cosmic-text` 渲染：

```bash
# 使用系统等宽字体（默认）
cargo run -p app-desktop

# 指定 OpenType 字体（.otf / .ttf / .ttc / .otc）
FONT_PATH="C:/Windows/Fonts/consola.ttf" cargo run -p app-desktop

# 关闭 FPS 叠加
CROSS_RENDER_FPS=0 cargo run -p app-desktop
```

字体只加载一次；`FONT_PATH` 读取失败或没有可用系统字体时只打印警告，不影响 3D 渲染。

## 导入自定义模型

支持 `.obj`、`.gltf`、`.glb`、`.pmx`，可用命令行参数或 `MODEL_PATH` 环境变量；VMD 动画用 `VMD_PATH` 或第二个命令行参数：

```bash
MODEL_PATH=models/cube.obj MAX_FRAMES=5 cargo run -p app-desktop
MODEL_PATH=models/triangle.gltf MAX_FRAMES=5 cargo run -p app-desktop
MODEL_PATH=models/triangle.pmx MAX_FRAMES=5 cargo run -p app-desktop
cargo run -p app-desktop -- path/to/your_model.pmx path/to/motion.vmd
```

输出会显示模型名、顶点/三角形/材质/贴图/骨骼数与动画轨道数。模型自动居中缩放，使用 Toon + 描边 + Sphere map 渲染，并可通过 VMD 驱动骨骼动画。未提供模型时使用内置三角形回退。

PMX/MMD 当前完成到 v0.7：几何、贴图材质、Toon/描边、骨骼蒙皮、VMD 动画。表情（v0.8）与刚体物理（v0.9）尚未实现，详见 `docs/MMD_ROADMAP.md`。

## 设计原则

1. 单一内核：平台 crate 只负责创建 surface 和驱动帧循环，渲染逻辑不重复。
2. 强制 Vulkan（桌面）：避免 DX12/dxc 额外依赖，复用已有 Vulkan 研究。
3. 无重型本地工具链：WGSL 直接编译，不需要 cmake / shaderc / glslc。
4. 分层而非单文件：workspace + lib + 平台 crate，带测试、fmt、clippy。

## 文档

- `docs/ARCHITECTURE.md`：架构与 API
- `docs/BUILD.md`：各平台构建命令
- `docs/PLATFORMS.md`：平台支持矩阵与验证状态
- `docs/MMD_ROADMAP.md`：完整 MMD 兼容迭代路线
