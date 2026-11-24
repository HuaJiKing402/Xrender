use wgpu::{
    BindGroup, Device, RenderPass, Texture, TextureFormat, VertexAttribute, VertexBufferLayout,
    VertexFormat, VertexStepMode,
};

use glam::Vec3;

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

/// 绘制所有不透明材质分组。
pub(crate) fn draw_opaque_groups<'a>(
    pass: &mut RenderPass<'a>,
    groups: &'a [MaterialGroup],
    bind_groups: &'a [BindGroup],
    material_transparent: &[bool],
) {
    for group in groups {
        if group.index_count == 0 {
            continue;
        }
        if material_transparent
            .get(group.material_index)
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        draw_group(pass, bind_groups, group);
    }
}

/// 按显式顺序绘制透明材质分组，调用方负责从远到近排序。
pub(crate) fn draw_transparent_groups_ordered<'a>(
    pass: &mut RenderPass<'a>,
    groups: &'a [MaterialGroup],
    bind_groups: &'a [BindGroup],
    order: &[usize],
) {
    for &index in order {
        let Some(group) = groups.get(index) else {
            continue;
        };
        if group.index_count == 0 {
            continue;
        }
        draw_group(pass, bind_groups, group);
    }
}

fn draw_group(pass: &mut RenderPass<'_>, bind_groups: &[BindGroup], group: &MaterialGroup) {
    if let Some(bind_group) = bind_groups.get(group.material_index) {
        pass.set_bind_group(1, bind_group, &[]);
    }
    pass.draw_indexed(
        group.index_start as u32..(group.index_start + group.index_count) as u32,
        0,
        0..1,
    );
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

/// 透明分组按相机距离从远到近排序。
pub(crate) fn transparent_order(
    groups: &[MaterialGroup],
    centers: &[Vec3],
    material_transparent: &[bool],
    eye: Vec3,
) -> Vec<usize> {
    let mut order: Vec<usize> = groups
        .iter()
        .enumerate()
        .filter(|(_, group)| {
            material_transparent
                .get(group.material_index)
                .copied()
                .unwrap_or(false)
        })
        .map(|(index, _)| index)
        .collect();
    order.sort_by(|&a, &b| {
        let distance_a = (centers[a] - eye).length_squared();
        let distance_b = (centers[b] - eye).length_squared();
        distance_b.total_cmp(&distance_a)
    });
    order
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
