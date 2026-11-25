use wgpu::util::DeviceExt;

use crate::model::{texture::TextureSlot, MaterialProfile, ShadingData};

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

/// 描边壳的膨胀量占模型尺寸的比例。
///
/// 模型在装载时已被归一化到单位尺寸，所以这里用固定比例即可；
/// 具体宽度 = 该比例 × 材质的 edge_scale，由 CPU 算好再传给 shader。
const OUTLINE_WIDTH_RATIO: f32 = 0.012;

/// 材质整体半透明（或 alpha 通道整体偏低）时才使用 blend；
/// 普通局部镂空走 alpha cutoff，避免大材质失去深度写入。
fn is_transparent(material: &MaterialProfile, texture_slots: &[TextureSlot]) -> bool {
    let texture_alpha_max = material
        .texture_index
        .and_then(|index| texture_slots.get(index))
        .map(|slot| slot.alpha_max)
        .unwrap_or(255);
    material.diffuse[3] < 0.999 || texture_alpha_max < 230
}

/// 解析 toon 贴图：MMD 的 `toon_reference` 决定查外部贴图表还是内置 toon 表。
///
/// `toon_index < 0` 在 PMX 里的语义是「该材质没有 toon」，此处返回 `None`，
/// 调用方会保留白色 1×1 贴图——采样得到 1.0，等于不改变颜色、也不产生阴影。
/// 之前这种情况会回退到 toon01，凭空给无 toon 材质加了一层灰影。
fn resolve_toon_slot<'a>(
    toon_reference: u8,
    toon_index: i32,
    texture_slots: &'a [TextureSlot],
    toon_slots: &'a [TextureSlot],
) -> Option<&'a TextureSlot> {
    // 显式无 toon：保持白色乘数，不要退回内置 toon。
    if toon_index < 0 {
        return None;
    }
    let index = toon_index as usize;
    if toon_reference == 0 {
        // 引用贴图表；索引越界时同样按"无 toon"处理，避免错用别的贴图当 ramp。
        texture_slots.get(index)
    } else {
        // 内置 toon01..10；越界时退到第一张作为兜底。
        toon_slots.get(index).or_else(|| toon_slots.first())
    }
}

pub(crate) fn create_material_state(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    ramp_sampler: &wgpu::Sampler,
    texture_slots: &[TextureSlot],
    toon_slots: &[TextureSlot],
    fallback_view: &wgpu::TextureView,
    materials: &[MaterialProfile],
) -> MaterialState {
    let transparent: Vec<bool> = materials
        .iter()
        .map(|material| is_transparent(material, texture_slots))
        .collect();
    let outline: Vec<bool> = materials
        .iter()
        .map(|material| material.outline.is_some())
        .collect();
    let bind_groups = materials
        .iter()
        .enumerate()
        .map(|(material_index, material)| {
            let blend = transparent[material_index];
            let (edge_color, edge_scale) = material
                .outline
                .as_ref()
                .map(|outline| (outline.color, outline.scale * OUTLINE_WIDTH_RATIO))
                .unwrap_or(([0.0, 0.0, 0.0, 1.0], 0.0));
            // 着色模型决定剩下的参数；默认值等价于"贴图 × diffuse"。
            let mut ambient = [0.0f32; 3];
            let mut toon_code = 0.0f32;
            let mut specular = [0.0f32; 4];
            let mut sphere_mode = 0.0f32;
            let mut sphere_enabled = false;
            let mut toon_view = fallback_view.clone();
            let mut sphere_view = fallback_view.clone();
            match &material.shading {
                ShadingData::Unlit => {}
                ShadingData::Mmd(mmd) => {
                    ambient = material.ambient;
                    specular = [
                        material.specular[0],
                        material.specular[1],
                        material.specular[2],
                        material.specular_strength,
                    ];
                    sphere_mode = f32::from(mmd.sphere_mode);
                    if let Some(slot) = resolve_toon_slot(
                        mmd.toon_reference,
                        mmd.toon_index,
                        texture_slots,
                        toon_slots,
                    ) {
                        toon_view = slot.view.clone();
                        toon_code = slot.toon.code();
                    }
                    if let Some(view) = mmd
                        .sphere_index
                        .and_then(|index| texture_slots.get(index))
                        .map(|slot| slot.view.clone())
                    {
                        sphere_view = view;
                        sphere_enabled = true;
                    }
                }
            }
            let uniform = MaterialUniform {
                diffuse: material.diffuse,
                edge_color,
                params: [
                    edge_scale,
                    sphere_mode,
                    if sphere_enabled { 1.0 } else { 0.0 },
                    if blend { 0.01 } else { 0.5 },
                ],
                // ambient.rgb 是 PMX 材质的环境/自发光色，参与光照加算；
                // ambient.w 复用为 toon 采样编码，见 `ToonSampling::code`。
                ambient: [ambient[0], ambient[1], ambient[2], toon_code],
                specular,
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
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(ramp_sampler),
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
