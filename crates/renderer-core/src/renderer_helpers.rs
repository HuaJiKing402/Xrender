use wgpu::{
    Device, Texture, TextureFormat, VertexAttribute, VertexBufferLayout, VertexFormat,
    VertexStepMode,
};

use glam::Vec3;

use crate::model::{MaterialGroup, MeshVertex};

pub(crate) const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;

const BLIT_SOURCE: &str = include_str!("shaders/blit.wgsl");

/// blit 的贴图绑定：group 0 只有一张离屏颜色 + 一个采样器，与场景的
/// uniform / 骨骼 / 材质三组无关，所以用独立的 layout。
pub(crate) fn blit_bind_group_layout(device: &Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("blit bind group layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

pub(crate) fn create_blit_pipeline(
    device: &Device,
    layout: &wgpu::BindGroupLayout,
    format: TextureFormat,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("blit.wgsl"),
        source: wgpu::ShaderSource::Wgsl(BLIT_SOURCE.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("blit pipeline layout"),
        bind_group_layouts: &[Some(layout)],
        ..Default::default()
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("blit pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

static ATTRIBUTES: [VertexAttribute; 6] = [
    VertexAttribute {
        format: VertexFormat::Float32x3,
        offset: 0,
        shader_location: 0,
    },
    VertexAttribute {
        format: VertexFormat::Float32x3,
        offset: 12,
        shader_location: 1,
    },
    VertexAttribute {
        format: VertexFormat::Float32x3,
        offset: 24,
        shader_location: 2,
    },
    VertexAttribute {
        format: VertexFormat::Float32x2,
        offset: 36,
        shader_location: 3,
    },
    VertexAttribute {
        format: VertexFormat::Uint32x4,
        offset: 44,
        shader_location: 4,
    },
    VertexAttribute {
        format: VertexFormat::Float32x4,
        offset: 60,
        shader_location: 5,
    },
];

pub(crate) fn vertex_layout() -> VertexBufferLayout<'static> {
    VertexBufferLayout {
        array_stride: std::mem::size_of::<MeshVertex>() as u64,
        step_mode: VertexStepMode::Vertex,
        attributes: &ATTRIBUTES,
    }
}

pub(crate) fn depth_state(write_depth: bool) -> wgpu::DepthStencilState {
    depth_state_with(write_depth, wgpu::CompareFunction::Less)
}

pub(crate) fn depth_state_with(
    write_depth: bool,
    compare: wgpu::CompareFunction,
) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write_depth),
        depth_compare: Some(compare),
        stencil: Default::default(),
        bias: Default::default(),
    }
}

/// 离屏场景颜色目标。格式与 surface 一致，这样 blit 不需要做格式转换，
/// 颜色在 sRGB 目标上做一次线性→编码的往返，结果与直接画到 surface 等同。
///
/// `samples > 1` 时这是多重采样目标，不能直接采样或拷贝，必须由
/// [`create_resolve_target`] 解析成单采样；此时也就不需要 `TEXTURE_BINDING`。
pub(crate) fn create_scene_color(
    device: &Device,
    width: u32,
    height: u32,
    format: TextureFormat,
    samples: u32,
) -> Texture {
    let usage = if samples > 1 {
        wgpu::TextureUsages::RENDER_ATTACHMENT
    } else {
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
    };
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene color"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples.max(1),
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// 多重采样目标的解析结果：单采样、可采样、可回读。
pub(crate) fn create_resolve_target(
    device: &Device,
    width: u32,
    height: u32,
    format: TextureFormat,
) -> Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scene resolve"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// 选多重采样级别：优先 4x，退 2x，都不支持就关掉。
///
/// 只读格式特性里的 `MULTISAMPLE_X*` 标志，不假设"桌面端一定有 4x"。
pub(crate) fn choose_sample_count(flags: wgpu::TextureFormatFeatureFlags) -> u32 {
    [4u32, 2]
        .into_iter()
        .find(|count| flags.sample_count_supported(*count))
        .unwrap_or(1)
}

pub(crate) fn create_depth_texture(
    device: &Device,
    width: u32,
    height: u32,
    samples: u32,
) -> Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples.max(1),
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// 计算每个材质分组的世界空间近似中心，用于透明排序。
pub(crate) fn group_centers(
    vertices: &[MeshVertex],
    indices: &[u32],
    groups: &[MaterialGroup],
) -> Vec<Vec3> {
    groups
        .iter()
        .map(|group| {
            let end = group.index_start + group.index_count;
            let mut sum = Vec3::ZERO;
            let mut count = 0u32;
            for index in &indices[group.index_start..end] {
                if let Some(vertex) = vertices.get(*index as usize) {
                    sum += Vec3::from_array(vertex.position);
                    count += 1;
                }
            }
            if count == 0 {
                Vec3::ZERO
            } else {
                sum / count as f32
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_layout_matches_mesh_vertex() {
        let layout = vertex_layout();
        assert_eq!(std::mem::size_of::<MeshVertex>(), 76);
        assert_eq!(layout.array_stride, 76);
        assert_eq!(layout.attributes.len(), 6);
        assert_eq!(layout.attributes[5].offset, 60);
    }
}
