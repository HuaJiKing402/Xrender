use std::{borrow::Cow, collections::HashMap};

use wgpu::{
    BlendState, ColorWrites, CompareFunction, Face, PipelineLayout, RenderPipeline, ShaderModule,
    TextureFormat,
};

use crate::renderer_helpers::{depth_state, depth_state_with, vertex_layout};

const TOON_SOURCE: &str = include_str!("shaders/toon.wgsl");
const OUTLINE_SOURCE: &str = include_str!("shaders/outline.wgsl");
const DEPTH_SOURCE: &str = include_str!("shaders/depth.wgsl");

/// 着色器种类。新增一种风格（PBR、Unlit……）只需要加一个变体和一份 WGSL。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ShadingModel {
    Toon,
    Outline,
    Depth,
}

impl ShadingModel {
    fn label(self) -> &'static str {
        match self {
            ShadingModel::Toon => "toon",
            ShadingModel::Outline => "outline",
            ShadingModel::Depth => "depth",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum BlendMode {
    Opaque,
    AlphaBlend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DepthMode {
    /// 写深度，`Less`：深度预通道。
    WriteLess,
    /// 只测不写，`LessEqual`：几何深度已经由预通道写好。
    TestLessEqual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CullMode {
    None,
    Front,
}

/// 一条管线的最小描述：所有固定功能状态都由它决定，因此可以按 key 缓存复用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PipelineKey {
    pub shading: ShadingModel,
    pub blend: BlendMode,
    pub depth: DepthMode,
    pub cull: CullMode,
    pub color_write: bool,
}

impl PipelineKey {
    fn label(self) -> String {
        format!(
            "{}-{:?}-{:?}-{:?}{}",
            self.shading.label(),
            self.blend,
            self.depth,
            self.cull,
            if self.color_write { "" } else { "-nocolor" }
        )
    }
}

/// 管线缓存：着色器模块在这里持有，管线按 key 惰性构建且只构建一次。
pub(crate) struct PipelineCache {
    toon: ShaderModule,
    outline: ShaderModule,
    depth: ShaderModule,
    entries: HashMap<PipelineKey, RenderPipeline>,
}

impl PipelineCache {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = |label: &str, source: &'static str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(source)),
            })
        };
        Self {
            toon: module("toon.wgsl", TOON_SOURCE),
            outline: module("outline.wgsl", OUTLINE_SOURCE),
            depth: module("depth.wgsl", DEPTH_SOURCE),
            entries: HashMap::new(),
        }
    }

    /// 保证某个 key 对应的管线存在，返回同一个 key 便于取用。
    /// 具体要哪些管线由 `pass::ScenePasses` 决定。
    pub fn ensure(
        &mut self,
        device: &wgpu::Device,
        layout: &PipelineLayout,
        format: TextureFormat,
        samples: u32,
        key: PipelineKey,
    ) -> PipelineKey {
        if !self.entries.contains_key(&key) {
            let pipeline = self.build(device, layout, format, samples, key);
            self.entries.insert(key, pipeline);
        }
        key
    }

    pub fn get(&self, key: PipelineKey) -> &RenderPipeline {
        self.entries
            .get(&key)
            .expect("pipeline must be ensured before use")
    }

    fn build(
        &self,
        device: &wgpu::Device,
        layout: &PipelineLayout,
        format: TextureFormat,
        samples: u32,
        key: PipelineKey,
    ) -> RenderPipeline {
        let module = match key.shading {
            ShadingModel::Toon => &self.toon,
            ShadingModel::Outline => &self.outline,
            ShadingModel::Depth => &self.depth,
        };
        let blend = match key.blend {
            BlendMode::Opaque => None,
            BlendMode::AlphaBlend => Some(BlendState::ALPHA_BLENDING),
        };
        let write_mask = if key.color_write {
            ColorWrites::ALL
        } else {
            ColorWrites::empty()
        };
        let cull_mode = match key.cull {
            CullMode::None => None,
            CullMode::Front => Some(Face::Front),
        };
        let depth_stencil = match key.depth {
            DepthMode::WriteLess => depth_state(true),
            DepthMode::TestLessEqual => depth_state_with(false, CompareFunction::LessEqual),
        };

        let label = format!("{} pipeline", key.label());
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label.as_str()),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_main"),
                buffers: &[Some(vertex_layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode,
                ..Default::default()
            },
            depth_stencil: Some(depth_stencil),
            multisample: wgpu::MultisampleState {
                count: samples.max(1),
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    }
}
