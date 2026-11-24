//! 帧内 pass 的顺序与职责。
//!
//! 每个 pass 由三件事描述：用哪条管线、画哪批材质分组、叫什么名字。
//! 这样加一个 pass（阴影、后处理、SSAO）只是往计划里插一条，不用改
//! `Renderer::render_frame` 里的字面代码。
//!
//! 深度依赖是这套顺序的硬约束：
//! - 有描边时，prepass 先用 `Less` 写深度（不写颜色），描边壳才能靠它做遮挡，
//!   之后的 toon pass 只测不写（`LessEqual`）。
//! - 没有描边时不需要 prepass，`opaque` pass 自己写深度（`Less`），
//!   省掉一整遍全量顶点蒙皮。

use wgpu::{TextureFormat, PipelineLayout};

use crate::pipeline::{BlendMode, CullMode, DepthMode, PipelineCache, PipelineKey, ShadingModel};

/// 一个 pass 负责画哪批分组。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassDraw {
    /// 不透明材质分组。
    OpaqueGroups,
    /// 有描边且整体不透明的分组。
    OutlineGroups,
    /// 按显式顺序绘制透明分组。
    TransparentGroupsOrdered,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScenePass {
    pub label: &'static str,
    pub pipeline: PipelineKey,
    pub draw: PassDraw,
}

/// 一帧的 pass 计划，按绘制顺序排列。
#[derive(Clone, Debug)]
pub(crate) struct ScenePasses {
    passes: Vec<ScenePass>,
}

impl ScenePasses {
    /// 根据场景内容（是否有描边）构建 pass 计划，同时保证对应管线存在。
    pub fn build(
        cache: &mut PipelineCache,
        device: &wgpu::Device,
        layout: &PipelineLayout,
        format: TextureFormat,
        has_outline: bool,
    ) -> Self {
        let mut passes = Vec::with_capacity(4);

        if has_outline {
            passes.push(ScenePass {
                label: "depth-prepass",
                pipeline: cache.ensure(
                    device,
                    layout,
                    format,
                    PipelineKey {
                        shading: ShadingModel::Depth,
                        blend: BlendMode::Opaque,
                        depth: DepthMode::WriteLess,
                        cull: CullMode::None,
                        color_write: false,
                    },
                ),
                draw: PassDraw::OpaqueGroups,
            });
            passes.push(ScenePass {
                label: "outline",
                pipeline: cache.ensure(
                    device,
                    layout,
                    format,
                    PipelineKey {
                        shading: ShadingModel::Outline,
                        blend: BlendMode::Opaque,
                        depth: DepthMode::TestLessEqual,
                        cull: CullMode::Front,
                        color_write: true,
                    },
                ),
                draw: PassDraw::OutlineGroups,
            });
        }

        // 没有 prepass 时，opaque 必须自己写深度，否则不透明面之间没有遮挡。
        let opaque_depth = if has_outline {
            DepthMode::TestLessEqual
        } else {
            DepthMode::WriteLess
        };
        passes.push(ScenePass {
            label: "opaque",
            pipeline: cache.ensure(
                device,
                layout,
                format,
                PipelineKey {
                    shading: ShadingModel::Toon,
                    blend: BlendMode::Opaque,
                    depth: opaque_depth,
                    cull: CullMode::None,
                    color_write: true,
                },
            ),
            draw: PassDraw::OpaqueGroups,
        });

        passes.push(ScenePass {
            label: "transparent",
            pipeline: cache.ensure(
                device,
                layout,
                format,
                PipelineKey {
                    shading: ShadingModel::Toon,
                    blend: BlendMode::AlphaBlend,
                    depth: DepthMode::TestLessEqual,
                    cull: CullMode::None,
                    color_write: true,
                },
            ),
            draw: PassDraw::TransparentGroupsOrdered,
        });

        Self { passes }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ScenePass> {
        self.passes.iter()
    }
}
