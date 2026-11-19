# MMD 完整兼容迭代路线

目标：把 CrossRender 从“能显示 PMX 几何”逐步迭代到完整 MMD 兼容运行时。

状态标记：✅ 已完成 / ⏳ 待实现。

## v0.3 — PMX 基础几何 ✅

- PMX 2.0 / 2.1 header、顶点、三角面、纹理表、材质块解析
- 顶点位置/法线/材质漫反射色 → 统一 `Mesh`
- 自动居中缩放、索引绘制、MVP 旋转动画

验收：`MODEL_PATH=models/triangle.pmx MAX_FRAMES=3 cargo run -p app-desktop`

## v0.4 — UV 与贴图材质 ✅

- `MeshVertex` 增加 `uv`，WGSL 增加纹理坐标
- 加载 PMX 纹理表，支持 PNG/BMP/TGA/JPG
- 材质贴图采样器与 BindGroup 扩展，缺贴图回退白色
- 深度缓冲解决 3D 自遮挡，按材质分组绘制
- 贴图路径按 PMX 编码解码，相对 PMX 所在目录解析

## v0.5 — Toon 与描边 ✅

- MMD 内部 toon01–toon10 渐变纹理与外部 toon 引用
- 背面膨胀描边 pass，材质边缘色与 edge scale
- Sphere map 乘算/加算

## v0.6 — 骨骼与蒙皮 ✅

- 解析 PMX 骨骼块：名称、父索引、位置、层级、标志位、IK 数据跳过
- 顶点权重：BDEF1 / BDEF2 / BDEF4 / SDEF / QDEF（QDEF 退化为 BDEF4）
- 骨骼矩阵 read-only storage buffer + GPU skinning
- 真实模型 622 骨骼解析与渲染通过

## v0.7 — VMD 动画 ✅

- VMD 骨骼/表情关键帧解析，Shift-JIS 文本处理
- 骨骼位置/四元数线性与 slerp 采样
- `Renderer::set_animation()` 与动画时钟驱动
- 最小 VMD 夹具：单骨骼旋转，本地真实模型 60 帧验证通过

## v0.8 — 表情与显示帧 ⏳

- PMX morph：group morph 与 vertex morph
- VMD 表情关键帧驱动
- 显示帧（DisplayFrame）UI 分组信息
- 表情权重叠加顺序

## v0.9 — 刚体物理 ⏳

- 解析 PMX 刚体与关节
- 接入 Rapier 或 Bullet 后端
- 骨骼物理联动与布料/头发模拟
- 自碰撞与关节约束

## v1.0 — 完整 MMD 场景 ⏳

- 多模型场景、相机/灯光动画统一时间轴
- 透明排序与后处理对齐 MMD 画质
- 一组真实 PMX/VMD 模型兼容性回归
- 桌面稳定发布；其他平台按 scaffolding 状态逐步跟进

## 当前边界

已完成 v0.4–v0.7，本轮目标达成。表情/物理/透明排序仍未实现，不把这些能力写成“已支持”。测试大模型与贴图/VMD 仅本地回归，不入库。
