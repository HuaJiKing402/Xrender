# Platforms

| 平台 | 后端 | 入口 crate | 验证状态 | 说明 |
|---|---|---|---|---|
| Windows 桌面 | Vulkan（强制） | `app-desktop` | **已在本机验证** | wgpu 30.0.2；PMX 贴图/Toon/描边/骨骼/VMD/轨道相机/OpenType FPS |
| Linux 桌面 | Vulkan（强制） | `app-desktop` | 脚手架 | 同一入口，需在 Linux 设备验证 |
| Android | Vulkan | `app-android` | 源码脚手架 | GameActivity + ANativeWindow，minSdk 26；已有 manifest/根 build.gradle，缺 Gradle wrapper、settings.gradle 与 app 模块目录 |
| macOS | Metal | `app-apple` | 源码脚手架 | 原生 Metal，不需要 MoltenVK；`platforms/apple/xcodegen.yml` 需 xcodegen 生成工程 |
| iOS | Metal | `app-apple` | 源码脚手架 | 需处理 surface 生命周期与签名；`Info.plist` 不是 Rust workspace 构建输入 |
| Web / WASM | WebGPU | `app-web` | 脚手架 | canvas + wasm-bindgen，浏览器需支持 WebGPU |

## 未验证项

- Linux/Android/macOS/iOS/Web 均未在本机运行；只保证代码结构、配置与文档齐全。
- WebGL2 降级不在首版范围。
- CI 编译矩阵不在首版范围。

## 版本锁定

- `wgpu 30.0.2`
- `winit 0.30.13`

升级任一项都需要重跑桌面硬验收命令。
