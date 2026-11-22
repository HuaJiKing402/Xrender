use wgpu::{
    BindGroup, Device, RenderPass, Texture, TextureFormat, VertexAttribute, VertexBufferLayout,
    VertexFormat, VertexStepMode,
};

use crate::model::{MaterialGroup, MeshVertex};

pub(crate) const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;

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
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write_depth),
        depth_compare: Some(wgpu::CompareFunction::Less),
        stencil: Default::default(),
        bias: Default::default(),
    }
}

pub(crate) fn create_depth_texture(device: &Device, width: u32, height: u32) -> Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// 按 material 分组绘制当前已绑定的 vertex/index buffer。
pub(crate) fn draw_material_groups<'a>(
    pass: &mut RenderPass<'a>,
    groups: &'a [MaterialGroup],
    bind_groups: &'a [BindGroup],
) {
    for group in groups {
        if group.index_count == 0 {
            continue;
        }
        if let Some(bind_group) = bind_groups.get(group.material_index) {
            pass.set_bind_group(1, bind_group, &[]);
        }
        pass.draw_indexed(
            group.index_start as u32..(group.index_start + group.index_count) as u32,
            0,
            0..1,
        );
    }
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
