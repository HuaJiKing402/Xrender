# CrossRender

一个用 Rust 写的多平台绘制引擎内核。首版以 `wgpu` 为唯一 GPU 后端，`winit` 处理窗口与事件，着色器统一用 WGSL；Windows 桌面强制 Vulkan。

## 当前状态

- `renderer-core`：平台无关渲染内核，负责 Instance/Adapter/Device、交换链配置、WGSL 图形管线、帧呈现。
- `app-desktop`：Windows/Linux 桌面入口，已在本机验证可渲染三角形。
- `app-web` / `app-android` / `app-apple`：Web、Android、Apple 平台脚手架（代码与配置齐全，未在本机验证）。

## 快速开始（Windows 桌面）

```bash
TRIANGLE_MAX_FRAMES=5 cargo run -p app-desktop
```

预期输出：

```text
Backend: Vulkan
Adapter: AMD Radeon RX 5700 XT
Window: 800x600
rendered 5 frames
```

窗口中出现一个粉红/渐变三角形。按 Esc 或关闭窗口退出；设置 `TRIANGLE_MAX_FRAMES` 后自动退出，便于 CI/脚本验证。

## 设计原则

1. 单一内核：平台 crate 只负责创建 surface 和驱动帧循环，渲染逻辑不重复。
2. 强制 Vulkan（桌面）：避免 DX12/dxc 额外依赖，复用已有 Vulkan 研究。
3. 无重型本地工具链：WGSL 直接编译，不需要 cmake / shaderc / glslc。
4. 分层而非单文件：workspace + lib + 平台 crate，带测试、fmt、clippy。

## 文档

- `docs/ARCHITECTURE.md`：架构与 API
- `docs/BUILD.md`：各平台构建命令
- `docs/PLATFORMS.md`：平台支持矩阵与验证状态
