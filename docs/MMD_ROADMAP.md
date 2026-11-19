# MMD 完整兼容迭代路线

目标：把 CrossRender 从“能显示 PMX 几何”逐步迭代到完整 MMD 兼容运行时。

## v0.3 — PMX 基础几何（当前）

- PMX 2.0 / 2.1 header、顶点、三角面、纹理表、材质块解析
- 顶点位置/法线/材质漫反射色 → 统一 `Mesh`
- 自动居中缩放、索引绘制、MVP 旋转动画
- 未处理：UV 使用、贴图、Toon、骨骼、表情、刚体

验收：`MODEL_PATH=models/triangle.pmx MAX_FRAMES=3 cargo run -p app-desktop`

## v0.4 — UV 与贴图材质

- `MeshVertex` 增加 `uv: [f32; 2]`，WGSL 增加纹理坐标
- 加载 PMX 纹理表，支持 PNG/BMP/TGA 常用格式
- 材质贴图采样器与 BindGroup 扩展
- 处理 sphere map（乘算/加算）

## v0.5 — Toon 与描边

- MMD Toon 渐变贴图（内部 toon01–toon10 / 自定义 toon）
- 两遍描边渲染（背面膨胀法）
- 材质边缘色与 edge scale

## v0.6 — 骨骼与蒙皮

- 解析 PMX 骨骼块：层级、旋转/移动/IK 标志
- GPU skinning：BDEF1 / BDEF2 / BDEF4 / SDEF / QDEF
- 骨骼矩阵纹理或 storage buffer
- 骨骼姿态更新与法线变换

## v0.7 — VMD 动画

- VMD 运动文件解析（骨骼关键帧、表情关键帧、相机、灯光）
- 动画采样与骨骼姿态插值
- 模型/动画绑定与播放控制

## v0.8 — 表情与显示帧

- PMX morph：group morph 与 vertex morph
- VMD 表情关键帧驱动
- 显示帧（DisplayFrame）UI 分组信息
- 表情权重叠加顺序

## v0.9 — 刚体物理

- 解析 PMX 刚体与关节
- 接入 Rapier 或 Bullet 后端
- 骨骼物理联动与布料/头发模拟
- 自碰撞与关节约束

## v1.0 — 完整 MMD 场景

- 多模型场景、相机/灯光动画统一时间轴
- 渲染质量对齐 MMD：Toon + 描边 + 透明排序 + 后处理
- 一组真实 PMX/VMD 模型的兼容性回归测试
- 桌面端稳定发布；其他平台按 scaffolding 状态逐步跟进

## 当前边界

完整兼容是长期目标，每个版本单独提交并验收；不把未验证的 MMD 能力写成“已支持”。
