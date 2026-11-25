# CrossRender

基于 `wgpu` 的多平台实时渲染引擎内核。一个平台无关的渲染核心，加上若干只负责「创建 surface + 驱动帧循环」的平台壳。

MMD（PMX / VMD）是第一个完整接入的格式与着色模型；它的专有逻辑正在从内核里逐步剥离，目标是让内核真正只认识「网格 + 材质 + 着色模型 + pass」，换模型格式或换着色风格不需要改渲染代码。

当前版本：workspace `0.1.0`，最近发布 tag `v0.7.4-cleanup`。

---

## 状态一览

| 能力 | 状态 | 说明 |
|---|---|---|
| 网格装载（OBJ / glTF / GLB） | ✅ | `model::load_from_path` |
| PMX 装载 | ✅ | 独立 `mmd` crate：几何、材质、骨骼、贴图路径 |
| VMD 动画 | ⚠️ | 能解析能播；按 `elapsed * 60` 采样，实际是 **2× 速**，到尾帧 clamp 不循环，贝塞尔插值未生效 |
| 骨骼蒙皮 | ✅ | CPU 算矩阵 + storage buffer，最多 4 权重 |
| Toon 明暗 | ✅ | ramp 查表 + 逐贴图轴向检测 + Clamp 采样 |
| 描边 | ✅ | 独立 pass，`cull Front`，跳过透明材质 |
| Sphere map | ⚠️ | 相机空间反射已修；采样器与 ramp 共用 Clamp，轮廓边缘会糊 |
| 高光 | ⚠️ | 有强度门控与指数钳制；不参与 ramp 调制 |
| 深度预通道 | ✅ | 仅在有描边的场景构建 |
| 半透明 | ⚠️ | 仅按材质组质心从远到近排序 |
| 离屏渲染 | ✅ | 场景画到 `scene_color`，再由全屏三角形 blit 到 surface |
| MSAA | ⚠️ | 场景目标按格式特性取 4x→2x→单采样；深度附件跟不上就整体退回 1。后处理与阴影仍未实现 |
| 后处理 / 阴影 | ❌ | 均未实现 |
| 多模型 / 场景树 | ❌ | `Renderer` 只持一份网格 |
| 无头渲染 | ✅ | `Renderer::new_headless` + `render_offscreen`，不需要窗口与交换链 |
| 画面读回 / 截图 | ✅ | `read_scene_rgba` 返回 RGBA8，`capture_png` 直接落盘 |
| 画面回归测试 | ✅ | `tests/golden.rs`，基线图在 `tests/golden/` |
| GPU 帧计时 | ⚠️ | 时间戳查询路径已就绪；本机驱动读回恒为 0，按「不可用」上报 |

---

## 平台

| 平台 | 后端 | 入口 crate | 验证状态 |
|---|---|---|---|
| Windows 桌面 | Vulkan（强制） | `app-desktop` | **已在本机验证** |
| Linux 桌面 | Vulkan | `app-desktop` | 代码可用，未验证 |
| Web (wasm32) | WebGPU | `app-web` | 仅渲染内置三角形，未验证 |
| Android | Vulkan / GLES | `app-android` | 缺 `settings.gradle`、wrapper 与 app 模块 |
| iOS / macOS | Metal | `app-apple` | 不成立：`winit` 无 iOS 后端，构建脚本只是把 `main.rs` 交给 xcodebuild |

---

## 快速开始

前置：Rust **1.89+**（toolchain 由 `rust-version` 约束），Windows 桌面需要支持 Vulkan 1.1 以上的驱动。

```bash
# 默认：内置三角形
cargo run -p app-desktop

# 指定模型（也可用 MODEL_PATH 环境变量）
cargo run -p app-desktop -- models/test_cube_stack.obj

# 指定模型 + 动作（也可用 VMD_PATH）
cargo run -p app-desktop -- path/to/model.pmx path/to/motion.vmd

# 渲染固定帧数后退出，适合脚本与回归
MAX_FRAMES=5 cargo run -p app-desktop -- models/test_cube.obj

# 跑完顺便存一张截图
MAX_FRAMES=5 CROSS_RENDER_CAPTURE=target/frame.png cargo run -p app-desktop -- models/test_cube_stack.obj
```

> Windows PowerShell 用 `$env:MAX_FRAMES='5'` 这类写法设置变量，`A=1 cmd` 的语法只在 bash 里成立。

典型输出：

```text
Model: test_cube_stack (72 vertices, 36 triangles, 1 materials, 0 textures, 0 bones)
Overlay: FPS
Backend: Vulkan
Adapter: AMD Radeon RX 5700 XT
Window: 800x600
rendered 5 frames
```

> 若本地 `~/.cargo/config.toml` 里配了 `rustc-wrapper = "sccache"` 但机器上没装 sccache，构建会报 `program not found`。临时清空即可：`RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WRAPPER= cargo build`。

测试：

```bash
cargo test --workspace
```

覆盖 OBJ 解析、内置网格、骨骼蒙皮、相机、FPS 统计、toon 轴向检测、PMX/VMD 解析、画面回读与像素封装、黄金图回归、GPU 计时契约，以及一个 Vulkan 适配器冒烟测试。

### 验证与回归

一个命令跑完格式、lint、构建、测试：

```powershell
pwsh -File scripts/check.ps1              # 全部
pwsh -File scripts/check.ps1 -SkipTests   # 没有 GPU 的机器上跳过画面测试
pwsh -File scripts/check.ps1 -UpdateGolden  # 改了着色后重生成基线图
```

| 手段 | 命令 | 判据 |
|---|---|---|
| 格式 | `cargo fmt --all --check` | 有差异即失败 |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` | 任何 warning 即失败 |
| 构建 | `cargo build --workspace` | — |
| 单元/集成 | `cargo test --workspace` | 28 个测试 |
| 画面回归 | `cargo test -p renderer-core --test golden` | 平均通道差 ≤ 3，且单像素差 > 8 的占比 ≤ 1% |

黄金图的基线是仓库里提交的 PNG（`tests/golden/`），由无头渲染生成：320×240、默认轨道相机再偏转成三分之四视角。改了着色确实该变画面时，用 `-UpdateGolden` 重生成，并把新图和代码一起提交、在提交信息里写清为什么变。

---

## 操作

| 输入 | 行为 |
|---|---|
| 左键拖拽 | 绕模型旋转 |
| 右键 / 中键拖拽 | 平移视口 |
| 滚轮 | 拉近 / 推远 |
| `R` | 复位相机 |
| `Esc` | 退出 |

交互模式按 `16.667 ms` 截止时间对齐，目标 60 fps；`MAX_FRAMES` 模式下不限速，用于跑帧与测耗时。

---

## 环境变量

| 变量 | 作用 |
|---|---|
| `MODEL_PATH` | 模型路径（等价于第一个命令行参数） |
| `VMD_PATH` | 动作路径（等价于第二个命令行参数） |
| `MAX_FRAMES` | 渲染 N 帧后自动退出；旧名 `TRIANGLE_MAX_FRAMES` 仍兼容 |
| `FONT_PATH` | FPS 叠加使用的 OpenType 字体 |
| `CROSS_RENDER_FPS=0` | 关闭 FPS 叠加 |
| `CROSS_RENDER_CAPTURE` | 跑完 `MAX_FRAMES` 后把帧存成 PNG 到该路径 |
| `CROSS_RENDER_GPU_TIME` | 跑完打印一次 GPU 帧耗时；驱动没有可用时钟时打印 `unavailable` |

`VMD_PATH` 不要设成空串——空串会被当成路径，报 `os error 3`。

---

## 模型与动画

| 格式 | 装载 | 支持内容 | 限制 |
|---|---|---|---|
| `.obj` | 内核 | `v` / `vt` / `vn` / `f`，多边形自动三角化 | 忽略 `mtl` 与 `usemtl`，无贴图 |
| `.gltf` / `.glb` | 内核 | 网格、索引、顶点法线 | 无材质与贴图 |
| `.pmx` | `mmd` crate | 几何、材质、贴图、骨骼、toon 索引 | 表情、刚体、IK 未实现 |
| `.vmd` | `mmd` crate | 骨骼旋转 / 平移轨道 | 2× 速、不循环、无混合 |

仓库内自带的测试模型：

| 文件 | 规模 | 用途 |
|---|---|---|
| `models/test_cube.obj` | 24 顶点 / 12 三角形 | 每面独立法线 + 完整 0..1 UV，验证朝向与 UV |
| `models/test_cube_stack.obj` | 72 顶点 / 36 三角形 | 三个互相穿插的立方体，验证深度、遮挡与描边壳 |
| `models/cube.obj` | 8 顶点 / 12 三角形 | 极简立方体，验证装载链路 |
| `models/triangle.gltf`、`models/triangle.pmx` | 单三角形 | 各格式的最小冒烟样例 |

`models/56e/`（MMD 测试模型与贴图）和 `models/M.obj`、`models/a.obj` 等大素材因体积与版权不入库。

---

## 项目结构

| crate | 职责 | 关键文件 |
|---|---|---|
| `renderer-core` | 平台无关内核：设备、surface、管线、材质、pass、帧呈现 | `gpu.rs`、`renderer.rs`、`pass.rs`、`pipeline.rs`、`material.rs`、`renderer_helpers.rs` |
| `mmd` | MMD 资源层：PMX / VMD 解析 | `pmx.rs`、`vmd.rs` |
| `app-desktop` | `winit` 窗口、输入、帧循环 | `main.rs` |
| `app-web` | wasm32 + canvas | `lib.rs` |
| `app-android` | `android_main` 入口 | `lib.rs` |
| `app-apple` | Apple 平台脚手架 | `main.rs` |

依赖方向单向：`app-* → renderer-core`，`mmd → renderer-core`，内核不反向依赖任何格式。

关键版本：`wgpu 30.0.2`、`winit 0.30.13`、`glyphon 0.12`（依赖 `cosmic-text 0.19`）、`glam 0.29`。

---

## 渲染管线

一帧的流程：

1. **CPU 准备**：采样动画 → 计算骨骼矩阵（622 骨约 40 KB）→ 写全局 uniform（MVP / model / 相机位置 / 视图旋转矩阵）→ 透明分组从远到近排序。
2. **场景 pass**（颜色目标是离屏 `scene_color`，与 surface 同格式；开了 MSAA 时先画进多采样目标 `scene_msaa`，pass 结束后 resolve 到 `scene_color`）：
   - `depth-prepass`：只在场景存在描边材质时构建。写深度、不写颜色，`Less`。
   - `outline`：`cull Front`，只测不写深度。跳过半透明材质。
   - `opaque`：有 prepass 时只测不写；没有 prepass 时自己写深度（`Less`）。
   - `transparent`：`ALPHA_BLENDING`，按材质组顺序绘制。
   - FPS 叠加：`glyphon`，在同一个 pass 末尾用 `depth Always` 绘制。
3. **blit pass**：全屏三角形把 `scene_color` 搬到 surface，1:1 采样，视觉结果与直画表面一致。

有窗口与无头两条路径共用同一段场景编码：`prepare_frame`（FPS 文本、动画姿态、uniforms）→ `encode_scene` → 有 surface 时再 `encode_blit`。无头模式停在 `encode_scene`，颜色留在 `scene_color` 里等回读；`capture_png` 就是"渲染一帧 → 读回 → 写盘"。

pass 的顺序、管线 key 和绘制范围由 `pass::ScenePasses` 描述，`render_frame` 只做分发；新增一个 pass 是在计划里插一条，而不是改动帧函数内部。

绑定布局：

| 组 | 内容 |
|---|---|
| `group 0` | 全局 uniform（208 B）+ 骨骼矩阵 storage buffer |
| `group 1` | 材质 uniform（80 B）+ diffuse 贴图/采样器 + toon 贴图 + sphere 贴图 + ramp 采样器 |

顶点格式 76 字节：`position` / `normal` / `color` / `uv` + 4×`u32` 骨骼索引 + 4×`f32` 权重。

---

## 着色与材质

**Toon 斜坡**：toon 图是明暗查找表，不是可平铺纹理。

- 采样坐标 `t = clamp(N·L * 0.5 + 0.5, 0, 1)`（半兰伯特）。
- 贴图的渐变轴与亮端位置在 CPU 侧用 16×16 降采样网格检测，编码成 4 种轴向，随 `ambient.w` 传给 shader。
- 采样器固定 `ClampToEdge`——用 Repeat 时端点会回卷，白端会串进暗部。
- 颜色计算是纯乘法：`base * ramp`。亮部白 → 保持原色，暗部影色 → 乘上影色，色阶数量决定阴影硬度。

**高光**：`specular_strength > 0` 才计算，指数钳制在 `[1, 100]`，与半程向量求幂后加到颜色上，出口统一 `clamp(0, 1)`。

**Sphere map**：法线转到相机空间，对视线方向 `(0,0,-1)` 求反射，`uv = (r.x*0.5+0.5, 0.5-r.y*0.5)`。模式 1 乘算、模式 2 加算。

**着色扩展槽**：`MaterialProfile.shading` 是 `ShadingData::Unlit` 或 `ShadingData::Toon(ToonParams)`。`ToonParams` 只说内核需要知道的事——用哪张 ramp（`RampSlot::Diffuse` 或 `Builtin`）、哪张环境球、乘算还是加算。PMX 的 `toon_reference`、索引 `-1`、内置表越界回落这些约定由 `mmd::pmx::toon_params` 翻译，内核代码里不再出现 MMD 的名字。

**贴图表**：`TextureTable` 持有模型贴图、ramp 表与一张白色兜底，`ramp_slot` / `diffuse_slot` / `white_view` 是唯一入口；材质构造只收这一个参数。

**描边**：沿蒙皮后的法线外扩出壳，宽度 = `0.012 × 材质 edge_scale`（CPU 侧算好绝对值），`cull Front` 只留背面壳，深度由 prepass 提供。

---

## 已知限制

1. 动画速度是名义值的 2 倍，且不循环（`clip.sample(elapsed * 60.0)` 而 VMD 是 30 fps 语义）。
2. 光照方向 `(0.4, 0.8, 1.0)` 硬编码在 `toon.wgsl`，没有光源抽象。
3. 没有阴影：无 shadow map、无自阴影。
4. 颜色空间规则不统一：贴图按 sRGB 解码，材质色与顶点色当线性值直接用。
5. 贴图无 mipmap、无压缩格式；14 张 800×800 在 debug 下启动约 9 秒。
6. MSAA 只在场景目标上开（4x→2x→关，由颜色格式特性决定）；blit 到 surface 仍是单采样，不改变最终呈现的采样数。
7. 单网格渲染，`model` 矩阵恒为单位阵；多模型需要重构。
8. 每个材质一个 uniform buffer 与一个 bind group，材质数量上升后绑定开销线性增长。
9. `cull Front` 对 PMX 双面薄片（`flags & 0x01`）无效，需要按材质分流管线。
10. Web / Android / iOS 均未在本机验证。
11. `Renderer` 仍是二十多个字段的大对象；`GpuContext` 与贴图表已经拆出去，多网格与资源句柄还没做。`docs/` 全部滞后于代码。
12. GPU 计时在本机（RX 5700 XT / Vulkan）读回恒为 0，接口对外报「不可用」；换设备或驱动才能拿到真实数字。
13. 黄金图基线是本机生成的，跨驱动的容差（平均差 3）刻意放宽，因此它对 1~2 级的颜色偏移不敏感。

---

## 开发路线

| 里程碑 | 版本 | 主题 | 核心交付 | 状态 |
|---|---|---|---|---|
| M0 | v0.8.0 | 可验证性 | 离屏读回、黄金图回归、`check` 脚本、GPU 计时 | **代码完成**（未打 tag） |
| M1 | v0.9.0 | 内核与资源分层 | `GpuContext`、资源句柄、多网格、完成 MMD 剥离 | 进行中：设备上下文、贴图表、`ShadingData::Toon`、骨骼精简已落地；多网格未开始 |
| M2 | v0.10.0 | 帧图与渲染特性 | 渲染图、MSAA、tonemap + AA、颜色空间策略 | 未开始 |
| M3 | v0.11.0 | 场景与光照 | 节点树、光源、阴影、剔除、材质变体 | 未开始 |
| M4 | v0.12.0 | 资产与动画 | 异步加载、mipmap、压缩纹理、动画修正 | 未开始 |
| M5 | v1.0.0 | 平台与发布 | 多平台验证、文档重写、版本与性能基线 | 未开始 |

每个里程碑以「先有验收手段，再谈功能」为原则；M0 排在最前就是为了让后续每一步都有可自动判定的依据。

---

## 文档

| 文件 | 内容 | 状态 |
|---|---|---|
| `docs/ARCHITECTURE.md` | 架构与 API | 滞后：未记录离屏目标、`pass.rs`、`mmd` crate、无头路径与回读接口 |
| `docs/BUILD.md` | 各平台构建 | 基本可用 |
| `docs/PLATFORMS.md` | 平台矩阵 | 基本可用 |
| `docs/MMD_ROADMAP.md` | MMD 兼容路线 | 部分过期 |

`docs/` 会在 M5 统一重写；在此之前，以本 README 与代码为准。

---

## 设计原则

1. **单一内核**：平台 crate 只创建 surface 并驱动帧循环，渲染逻辑不重复。
2. **不另建 RHI 层**：`wgpu` 就是抽象边界，分层只做「资源 / 场景 / 帧图」。
3. **无重型工具链**：WGSL 直接编译，不需要 cmake / shaderc / glslc；desktop 强制 Vulkan 以避开 dxc 依赖。
4. **每一步可独立回滚**：功能与修正按最小可验证单元提交，历史保持线性可读。
