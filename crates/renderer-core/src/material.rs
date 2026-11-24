use wgpu::util::DeviceExt;

use crate::model::{texture::TextureSlot, Material};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct MaterialUniform {
    pub diffuse: [f32; 4],
    pub edge_color: [f32; 4],
    pub params: [f32; 4],
    pub ambient: [f32; 4],
    pub specular: [f32; 4],
}

/// 每个材质的 GPU 绑定与透明/描边分类结果。
pub(crate) struct MaterialState {
    pub bind_groups: Vec<wgpu::BindGroup>,
    pub transparent: Vec<bool>,
    pub outline: Vec<bool>,
}

/// 材质整体半透明（或 alpha 通道整体偏低）时才使用 blend；
/// 普通局部镂空走 alpha cutoff，避免大材质失去深度写入。
fn is_transparent(material: &Material, texture_slots: &[TextureSlot]) -> bool {
    let texture_alpha_max = material
        .texture_index
        .and_then(|index| texture_slots.get(index))
        .map(|slot| slot.alpha_max)
        .unwrap_or(255);
    material.diffuse[3] < 0.999 || texture_alpha_max < 230
}

pub(crate) fn create_material_state(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    texture_slots: &[TextureSlot],
    toon_slots: &[TextureSlot],
    fallback_view: &wgpu::TextureView,
    materials: &[Material],
) -> MaterialState {
    let transparent: Vec<bool> = materials
        .iter()
        .map(|material| is_transparent(material, texture_slots))
        .collect();
    let outline: Vec<bool> = materials
        .iter()
        .map(|material| material.flags & 0x10 != 0)
        .collect();
    let bind_groups = materials
        .iter()
        .enumerate()
        .map(|(material_index, material)| {
            let blend = transparent[material_index];
            let sphere_view = material
                .sphere_index
                .and_then(|index| texture_slots.get(index))
                .map(|slot| slot.view.clone());
            let sphere_enabled = sphere_view.is_some();
            let uniform = MaterialUniform {
                diffuse: material.diffuse,
                edge_color: material.edge_color,
                params: [
                    material.edge_scale,
                    material.sphere_mode as f32,
                    if sphere_enabled { 1.0 } else { 0.0 },
                    if blend { 0.01 } else { 0.5 },
                ],
                ambient: [
                    material.ambient[0],
                    material.ambient[1],
                    material.ambient[2],
                    0.0,
                ],
                specular: [
                    material.specular[0],
                    material.specular[1],
                    material.specular[2],
                    material.specular_strength,
                ],
            };
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("material uniform"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let diffuse_view = material
                .texture_index
                .and_then(|index| texture_slots.get(index))
                .map(|slot| slot.view.clone())
                .unwrap_or_else(|| fallback_view.clone());
            let toon_view = if material.toon_reference == 0 {
                usize::try_from(material.toon_index)
                    .ok()
                    .and_then(|index| texture_slots.get(index))
            } else {
                usize::try_from(material.toon_index)
                    .ok()
                    .and_then(|index| toon_slots.get(index))
            }
            .map(|slot| slot.view.clone())
            .unwrap_or_else(|| fallback_view.clone());
            let sphere_view = sphere_view.unwrap_or_else(|| fallback_view.clone());

            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("material {}", material.name)),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&diffuse_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&toon_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&sphere_view),
                    },
                ],
            })
        })
        .collect();

    MaterialState {
        bind_groups,
        transparent,
        outline,
    }
}
