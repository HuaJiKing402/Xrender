//! 场景：GPU 上的网格资源，以及引用它们的实例。
//!
//! 这一层把"资源"和"出现在画面里的东西"分开：`GpuMesh` 是一份上传一次、
//! 可以被多个实例共享的顶点/索引/材质数据；`Instance` 只是"用哪份资源、
//! 放在什么变换下、放哪个动画"，它自己那份 GPU 数据只有一块骨骼矩阵 buffer
//! 和一个每对象 uniform。
//!
//! 这与 Unity 的 `Mesh` + `MeshFilter`/`MeshRenderer`、UE 的 `UStaticMesh` +
//! `UStaticMeshComponent` 是同一种分工：同模多实例不重复上传几何。

use std::path::Path;

use anyhow::{anyhow, bail, Result};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::{
    animation::AnimationClip,
    material::create_material_state,
    model::{texture::TextureTable, Bone, MaterialGroup, Mesh},
    renderer_helpers::group_centers,
    skinning::{compute_bone_matrices, Pose},
};

/// 传给 shader 的全局 uniform（208 B），与 `shaders/toon.wgsl` 的布局一一对应。
///
/// `mvp` 与 `model` 是每对象量，其余三项全帧共享；两者放在同一个 buffer 里，
/// 是因为第一版多实例先按对象各建一份 uniform，等对象数上来再拆成
/// "帧常量 + 对象索引"两张 buffer。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    camera_position: [f32; 4],
    /// 只有 toon.wgsl 读取（sphere map 需要在相机空间算反射）。
    view_rotation: [[f32; 4]; 4],
    /// 方向光颜色 × 强度；rgb 参与漫反射，a 预留。
    light: [f32; 4],
}

/// 全帧共享的那部分帧状态：相机位置、视图旋转、光照。
#[derive(Clone, Copy)]
pub(crate) struct FrameUniforms {
    pub camera_position: [f32; 4],
    pub view_rotation: [[f32; 4]; 4],
    pub light: [f32; 4],
}

fn matrices_to_floats(matrices: &[Mat4]) -> Vec<f32> {
    matrices
        .iter()
        .flat_map(|matrix| matrix.to_cols_array())
        .collect()
}

/// 一份上传到 GPU 的网格资源。
pub(crate) struct GpuMesh {
    name: String,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    /// 静止姿态的骨骼层级；实例的动画按它求矩阵。
    bones: Vec<Bone>,
    groups: Vec<MaterialGroup>,
    /// 每个材质分组在**局部空间**的近似中心，实例排序时再乘变换。
    group_centers: Vec<Vec3>,
    index_count: u32,
    material_bind_groups: Vec<wgpu::BindGroup>,
    material_transparent: Vec<bool>,
    material_outline: Vec<bool>,
    /// 这份资源用到的全部贴图；同模多实例共享这一份。
    textures: TextureTable,
}

impl GpuMesh {
    pub(crate) fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: Mesh,
        texture_base_dir: &Path,
        material_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        ramp_sampler: &wgpu::Sampler,
    ) -> Result<Self> {
        if !mesh.indices.len().is_multiple_of(3) {
            bail!("mesh index count must be a multiple of 3");
        }
        if mesh.indices.is_empty() {
            bail!("mesh has no indices");
        }

        let textures = TextureTable::load(device, queue, texture_base_dir, &mesh.texture_paths)?;
        let material_state = create_material_state(
            device,
            material_layout,
            sampler,
            ramp_sampler,
            &textures,
            &mesh.materials,
        );
        let group_centers = group_centers(&mesh.vertices, &mesh.indices, &mesh.groups);
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Ok(Self {
            name: mesh.name,
            vertex_buffer,
            index_buffer,
            bones: mesh.bones,
            groups: mesh.groups,
            group_centers,
            index_count: mesh.indices.len() as u32,
            material_bind_groups: material_state.bind_groups,
            material_transparent: material_state.transparent,
            material_outline: material_state.outline,
            textures,
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn textures(&self) -> &TextureTable {
        &self.textures
    }

    pub(crate) fn bones(&self) -> &[Bone] {
        &self.bones
    }

    pub(crate) fn bone_count(&self) -> usize {
        self.bones.len()
    }

    pub(crate) fn triangle_count(&self) -> u32 {
        self.index_count / 3
    }

    /// 这份资源里有没有材质开了描边；决定要不要建深度预通道。
    pub(crate) fn has_outline(&self) -> bool {
        self.material_outline.iter().any(|enabled| *enabled)
    }

    pub(crate) fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
    }

    /// 绘制所有不透明材质分组。
    pub(crate) fn draw_opaque(&self, pass: &mut wgpu::RenderPass<'_>) {
        for group in &self.groups {
            if self.skip_group(group) || self.is_transparent(group) {
                continue;
            }
            self.draw_group(pass, group);
        }
    }

    /// 只绘制启用了描边、且整体不透明的材质分组。
    ///
    /// 半透明材质如果也画描边，会在本体后面多出一层实色壳，看起来像脏边。
    pub(crate) fn draw_outline(&self, pass: &mut wgpu::RenderPass<'_>) {
        for group in &self.groups {
            if self.skip_group(group) || self.is_transparent(group) {
                continue;
            }
            if !self
                .material_outline
                .get(group.material_index)
                .copied()
                .unwrap_or(false)
            {
                continue;
            }
            self.draw_group(pass, group);
        }
    }

    /// 按给定顺序绘制透明材质分组；顺序由调用方跨实例统一排序。
    pub(crate) fn draw_transparent(&self, pass: &mut wgpu::RenderPass<'_>, order: &[usize]) {
        for &index in order {
            let Some(group) = self.groups.get(index) else {
                continue;
            };
            if self.skip_group(group) {
                continue;
            }
            self.draw_group(pass, group);
        }
    }

    /// 某个分组在实例世界空间下的近似中心，用于透明排序。
    pub(crate) fn group_center(&self, group_index: usize, transform: Mat4) -> Vec3 {
        self.group_centers
            .get(group_index)
            .map(|center| transform.transform_point3(*center))
            .unwrap_or(Vec3::ZERO)
    }

    /// 透明分组的索引。
    pub(crate) fn transparent_groups(&self) -> impl Iterator<Item = usize> + '_ {
        self.groups
            .iter()
            .enumerate()
            .filter(|(_, group)| self.is_transparent(group) && !self.skip_group(group))
            .map(|(index, _)| index)
    }

    fn skip_group(&self, group: &MaterialGroup) -> bool {
        group.index_count == 0
    }

    fn is_transparent(&self, group: &MaterialGroup) -> bool {
        self.material_transparent
            .get(group.material_index)
            .copied()
            .unwrap_or(false)
    }

    fn draw_group(&self, pass: &mut wgpu::RenderPass<'_>, group: &MaterialGroup) {
        if let Some(bind_group) = self.material_bind_groups.get(group.material_index) {
            pass.set_bind_group(1, bind_group, &[]);
        }
        pass.draw_indexed(
            group.index_start as u32..(group.index_start + group.index_count) as u32,
            0,
            0..1,
        );
    }
}

/// 场景里的一个实例：引用一份资源，加自己的变换、动画和每对象 GPU 数据。
pub(crate) struct Instance {
    /// 指向 `Scene` 里第几份资源。
    mesh: usize,
    transform: Mat4,
    animation: Option<AnimationClip>,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// 这个实例自己的骨骼矩阵；同模的两个实例可以摆不同姿势。
    bone_buffer: wgpu::Buffer,
}

impl Instance {
    pub(crate) fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        mesh_index: usize,
        mesh: &GpuMesh,
        transform: Mat4,
    ) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("object uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bone_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bone matrices"),
            contents: bytemuck::cast_slice(&matrices_to_floats(&rest_matrices(mesh))),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("object bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bone_buffer.as_entire_binding(),
                },
            ],
        });
        Self {
            mesh: mesh_index,
            transform,
            animation: None,
            uniform_buffer,
            bind_group,
            bone_buffer,
        }
    }

    pub(crate) fn mesh(&self) -> usize {
        self.mesh
    }

    pub(crate) fn transform(&self) -> Mat4 {
        self.transform
    }

    pub(crate) fn set_transform(&mut self, transform: Mat4) {
        self.transform = transform;
    }

    pub(crate) fn animation(&self) -> Option<&AnimationClip> {
        self.animation.as_ref()
    }

    pub(crate) fn set_animation(&mut self, clip: Option<AnimationClip>) {
        self.animation = clip;
    }

    pub(crate) fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// 按这个实例的变换写入每对象 uniform。
    pub(crate) fn write_uniforms(
        &self,
        queue: &wgpu::Queue,
        frame: &FrameUniforms,
        view_projection: Mat4,
    ) {
        let mvp = view_projection * self.transform;
        let uniforms = Uniforms {
            mvp: mvp.to_cols_array_2d(),
            model: self.transform.to_cols_array_2d(),
            camera_position: frame.camera_position,
            view_rotation: frame.view_rotation,
            light: frame.light,
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// 应用一个姿态到这个实例的骨骼矩阵。
    pub(crate) fn apply_pose(&self, queue: &wgpu::Queue, bones: &[Bone], pose: &Pose) {
        let mut matrices = compute_bone_matrices(bones, pose);
        if matrices.is_empty() {
            matrices.push(Mat4::IDENTITY);
        }
        queue.write_buffer(
            &self.bone_buffer,
            0,
            bytemuck::cast_slice(&matrices_to_floats(&matrices)),
        );
    }
}

/// 静止姿态的骨骼矩阵；网格没有骨骼时用单位阵占位，
/// 否则 storage buffer 尺寸为 0，绑定时会报错。
fn rest_matrices(mesh: &GpuMesh) -> Vec<Mat4> {
    let mut matrices = compute_bone_matrices(&mesh.bones, &Pose::default());
    if matrices.is_empty() {
        matrices.push(Mat4::IDENTITY);
    }
    matrices
}

/// 按到相机的距离从远到近排序透明分组；跨实例统一排，而不是每个实例各排一遍。
///
/// 只排序透明分组——不透明分组有深度测试兜底，顺序不影响结果。
pub(crate) fn transparent_draw_order(
    meshes: &[GpuMesh],
    instances: &[Instance],
    eye: Vec3,
) -> Vec<(usize, usize)> {
    let mut draws = Vec::new();
    for (instance_index, instance) in instances.iter().enumerate() {
        let Some(mesh) = meshes.get(instance.mesh()) else {
            continue;
        };
        for group_index in mesh.transparent_groups() {
            let distance =
                (mesh.group_center(group_index, instance.transform()) - eye).length_squared();
            draws.push((instance_index, group_index, distance));
        }
    }
    draws.sort_by(|a, b| b.2.total_cmp(&a.2));
    draws
        .into_iter()
        .map(|(instance, group, _)| (instance, group))
        .collect()
}

/// 校验一个网格能不能上传，返回失败原因。
pub(crate) fn validate_mesh(mesh: &Mesh) -> Result<()> {
    if !mesh.indices.len().is_multiple_of(3) {
        return Err(anyhow!("mesh index count must be a multiple of 3"));
    }
    if mesh.indices.is_empty() {
        return Err(anyhow!("mesh has no indices"));
    }
    Ok(())
}
