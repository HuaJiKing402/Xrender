use anyhow::{anyhow, Result};

pub mod gltf;
pub mod obj;
pub mod pmx;
pub mod texture;

pub use obj::load_obj;

/// 引擎统一顶点格式：位置、法线、颜色。
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    pub uv: [f32; 2],
    pub bone_indices: [u32; 4],
    pub bone_weights: [f32; 4],
}

/// PMX 骨骼（v0.6 用于蒙皮）。
#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    pub position: [f32; 3],
    pub layer: i32,
    pub flags: u16,
}

/// PMX 材质参数（v0.4 先使用 diffuse/texture，其他字段为后续 Toon 阶段准备）。
#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub diffuse: [f32; 4],
    pub ambient: [f32; 3],
    pub specular: [f32; 3],
    pub specular_strength: f32,
    pub texture_index: Option<usize>,
    pub sphere_index: Option<usize>,
    pub sphere_mode: u8,
    pub toon_reference: u8,
    pub toon_index: i32,
    pub flags: u8,
    pub edge_color: [f32; 4],
    pub edge_scale: f32,
}

impl Material {
    pub fn opaque() -> Self {
        Self {
            name: "default".into(),
            diffuse: [1.0, 1.0, 1.0, 1.0],
            ambient: [0.0, 0.0, 0.0],
            specular: [0.0, 0.0, 0.0],
            specular_strength: 0.0,
            texture_index: None,
            sphere_index: None,
            sphere_mode: 0,
            toon_reference: 0,
            toon_index: 0,
            flags: 0,
            edge_color: [0.0, 0.0, 0.0, 1.0],
            edge_scale: 1.0,
        }
    }
}

/// 材质对应的索引区间。
#[derive(Clone, Debug)]
pub struct MaterialGroup {
    pub material_index: usize,
    pub index_start: usize,
    pub index_count: usize,
}

/// 索引化的三角网格。
#[derive(Clone, Debug)]
pub struct Mesh {
    pub name: String,
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    pub materials: Vec<Material>,
    pub groups: Vec<MaterialGroup>,
    pub texture_paths: Vec<String>,
    pub bones: Vec<Bone>,
}

impl Mesh {
    /// 内置回退三角形（无需外部模型文件）。
    pub fn triangle() -> Self {
        let normal = [0.0, 0.0, 1.0];
        Self {
            name: "builtin-triangle".into(),
            vertices: vec![
                MeshVertex {
                    position: [0.0, 0.6, 0.0],
                    normal,
                    color: [0.98, 0.44, 0.52],
                    uv: [0.5, 0.0],
                    bone_indices: [0; 4],
                    bone_weights: [1.0, 0.0, 0.0, 0.0],
                },
                MeshVertex {
                    position: [-0.6, -0.5, 0.0],
                    normal,
                    color: [0.30, 0.70, 1.00],
                    uv: [0.0, 1.0],
                    bone_indices: [0; 4],
                    bone_weights: [1.0, 0.0, 0.0, 0.0],
                },
                MeshVertex {
                    position: [0.6, -0.5, 0.0],
                    normal,
                    color: [1.00, 0.86, 0.35],
                    uv: [1.0, 1.0],
                    bone_indices: [0; 4],
                    bone_weights: [1.0, 0.0, 0.0, 0.0],
                },
            ],
            indices: vec![0, 1, 2],
            materials: vec![Material::opaque()],
            groups: vec![MaterialGroup {
                material_index: 0,
                index_start: 0,
                index_count: 3,
            }],
            texture_paths: Vec::new(),
            bones: Vec::new(),
        }
    }

    /// 为没有材质信息的格式补一个覆盖全部索引的默认材质。
    pub fn ensure_default_materials(&mut self) -> &mut Self {
        if self.materials.is_empty() {
            self.materials.push(Material::opaque());
        }
        if self.groups.is_empty() {
            self.groups.push(MaterialGroup {
                material_index: 0,
                index_start: 0,
                index_count: self.indices.len(),
            });
        }
        self
    }

    /// 把模型居中并缩放到单位尺寸，方便任意自定义模型直接显示。
    pub fn normalize(&mut self) -> &mut Self {
        if self.vertices.is_empty() {
            return self;
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for vertex in &self.vertices {
            for axis in 0..3 {
                min[axis] = min[axis].min(vertex.position[axis]);
                max[axis] = max[axis].max(vertex.position[axis]);
            }
        }
        let center = [0usize, 1, 2].map(|i| (min[i] + max[i]) * 0.5);
        let extent = [0usize, 1, 2]
            .map(|i| max[i] - min[i])
            .into_iter()
            .fold(0.0_f32, f32::max)
            .max(f32::EPSILON);
        let scale = 1.4 / extent;
        for vertex in &mut self.vertices {
            for (axis, center_value) in center.iter().enumerate() {
                vertex.position[axis] = (vertex.position[axis] - *center_value) * scale;
            }
        }
        for bone in &mut self.bones {
            for (axis, center_value) in center.iter().enumerate() {
                bone.position[axis] = (bone.position[axis] - *center_value) * scale;
            }
        }
        self
    }

    /// PMX 等已有法线的模型：补默认材质并归一化，不改法线。
    pub(crate) fn finish(mut self) -> Self {
        self.ensure_default_materials();
        self.normalize();
        self
    }

    /// OBJ/glTF 等可能缺法线的模型：先补法线，再补默认材质并归一化。
    pub(crate) fn finish_with_normals(mut self) -> Self {
        self.ensure_normals();
        self.finish()
    }

    /// 若模型没有法线，按面法线补齐。
    pub fn ensure_normals(&mut self) {
        if self.vertices.iter().all(|v| v.normal != [0.0; 3]) {
            return;
        }
        for triangle in self.indices.as_chunks::<3>().0 {
            let (a, b, c) = (triangle[0], triangle[1], triangle[2]);
            let Some((p0, p1, p2)) = (|| {
                Some((
                    self.vertices.get(a as usize)?.position,
                    self.vertices.get(b as usize)?.position,
                    self.vertices.get(c as usize)?.position,
                ))
            })() else {
                continue;
            };
            let u = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let v = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2])
                .sqrt()
                .max(f32::EPSILON);
            let n = [n[0] / len, n[1] / len, n[2] / len];
            for index in triangle {
                if let Some(vertex) = self.vertices.get_mut(*index as usize) {
                    vertex.normal = n;
                    vertex.color = [0.5 + 0.5 * n[0], 0.5 + 0.5 * n[1], 0.75];
                }
            }
        }
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

pub(crate) fn color_from_normal(normal: [f32; 3]) -> [f32; 3] {
    [
        0.45 + 0.35 * normal[0].abs(),
        0.55 + 0.30 * normal[1].abs(),
        0.75 + 0.20 * normal[2].abs(),
    ]
}

/// 按扩展名加载自定义模型：OBJ / glTF / GLB / PMX。
#[cfg(not(target_arch = "wasm32"))]
pub fn load_from_path(path: impl AsRef<std::path::Path>) -> Result<Mesh> {
    let path = path.as_ref();
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| anyhow!("model path has no extension: {}", path.display()))?;
    match extension.as_str() {
        "obj" => obj::load_from_path(path),
        "gltf" | "glb" => gltf::load_from_path(path),
        "pmx" => pmx::load_pmx_from_path(path),
        other => Err(anyhow!(
            "unsupported model format: .{other} (use obj/gltf/glb/pmx)"
        )),
    }
}
