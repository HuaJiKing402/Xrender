use anyhow::{anyhow, Context, Result};

pub mod pmx;
pub mod texture;

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

fn resolve_index(raw: i64, len: usize) -> Result<usize> {
    let index = if raw > 0 {
        raw as usize - 1
    } else if raw < 0 {
        len.checked_add(raw as usize)
            .ok_or_else(|| anyhow!("invalid negative index"))?
    } else {
        return Err(anyhow!("OBJ index 0 is invalid"));
    };
    if index < len {
        Ok(index)
    } else {
        Err(anyhow!("index {raw} out of range (max {len})"))
    }
}

fn color_from_normal(normal: [f32; 3]) -> [f32; 3] {
    [
        0.45 + 0.35 * normal[0].abs(),
        0.55 + 0.30 * normal[1].abs(),
        0.75 + 0.20 * normal[2].abs(),
    ]
}

/// 解析 Wavefront OBJ（支持 v / vn / f，自动三角化多边形）。
pub fn load_obj(source: &str, name: impl Into<String>) -> Result<Mesh> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut vertices: Vec<MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for (line_number, line) in source.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("v") => {
                let x: f32 = parts.next().context("missing v.x")?.parse()?;
                let y: f32 = parts.next().context("missing v.y")?.parse()?;
                let z: f32 = parts.next().context("missing v.z")?.parse()?;
                positions.push([x, y, z]);
            }
            Some("vt") => {
                let u: f32 = parts.next().context("missing vt.u")?.parse()?;
                let v: f32 = parts.next().context("missing vt.v")?.parse()?;
                uvs.push([u, v]);
            }
            Some("vn") => {
                let x: f32 = parts.next().context("missing vn.x")?.parse()?;
                let y: f32 = parts.next().context("missing vn.y")?.parse()?;
                let z: f32 = parts.next().context("missing vn.z")?.parse()?;
                let len = (x * x + y * y + z * z).sqrt().max(f32::EPSILON);
                normals.push([x / len, y / len, z / len]);
            }
            Some("f") => {
                let corners: Vec<(usize, Option<usize>, Option<usize>)> = parts
                    .map(|token| {
                        let mut fields = token.split('/');
                        let p: i64 = fields.next().context("missing face position")?.parse()?;
                        let uv: Option<i64> = match fields.next() {
                            Some(value) if !value.is_empty() => Some(value.parse()?),
                            _ => None,
                        };
                        let n: Option<i64> = match fields.next() {
                            Some(value) if !value.is_empty() => Some(value.parse()?),
                            _ => None,
                        };
                        Ok((
                            resolve_index(p, positions.len())?,
                            match uv {
                                Some(uv) => Some(resolve_index(uv, uvs.len())?),
                                None => None,
                            },
                            match n {
                                Some(n) => Some(resolve_index(n, normals.len())?),
                                None => None,
                            },
                        ))
                    })
                    .collect::<Result<Vec<_>>>()
                    .with_context(|| format!("invalid face on line {}", line_number + 1))?;
                if corners.len() < 3 {
                    return Err(anyhow!(
                        "face on line {} has fewer than 3 corners",
                        line_number + 1
                    ));
                }
                let base = vertices.len() as u32;
                for (position_index, uv_index, normal_index) in &corners {
                    let normal = normal_index.map(|i| normals[i]).unwrap_or([0.0; 3]);
                    let uv = uv_index.map(|i| uvs[i]).unwrap_or([0.0, 0.0]);
                    vertices.push(MeshVertex {
                        position: positions[*position_index],
                        normal,
                        color: if normal == [0.0; 3] {
                            [0.75, 0.75, 0.8]
                        } else {
                            color_from_normal(normal)
                        },
                        uv,
                        bone_indices: [0; 4],
                        bone_weights: [1.0, 0.0, 0.0, 0.0],
                    });
                }
                for i in 1..corners.len() - 1 {
                    indices.push(base);
                    indices.push(base + i as u32);
                    indices.push(base + i as u32 + 1);
                }
            }
            _ => {}
        }
    }

    if positions.is_empty() || indices.is_empty() {
        return Err(anyhow!("OBJ contains no triangles"));
    }

    let mesh = Mesh {
        name: name.into(),
        vertices,
        indices,
        materials: Vec::new(),
        groups: Vec::new(),
        texture_paths: Vec::new(),
        bones: Vec::new(),
    };
    Ok(mesh.finish_with_normals())
}

#[cfg(not(target_arch = "wasm32"))]
fn load_from_obj_path(path: &std::path::Path) -> Result<Mesh> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("obj-model");
    load_obj(&source, name)
}

#[cfg(not(target_arch = "wasm32"))]
fn load_from_gltf_path(path: &std::path::Path) -> Result<Mesh> {
    let (document, buffers, _) =
        gltf::import(path).with_context(|| format!("failed to import {}", path.display()))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("gltf-model");

    for mesh in document.meshes() {
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = primitive
                .reader(|buffer| buffers.get(buffer.index()).map(|data| data.0.as_slice()));
            let Some(positions) = reader.read_positions() else {
                continue;
            };
            let positions: Vec<[f32; 3]> = positions.collect();
            let normals: Option<Vec<[f32; 3]>> = reader.read_normals().map(|n| n.collect());
            let tex_coords: Option<Vec<[f32; 2]>> =
                reader.read_tex_coords(0).map(|coords| match coords {
                    gltf::mesh::util::ReadTexCoords::U8(iter) => iter
                        .map(|uv| [uv[0] as f32 / u8::MAX as f32, uv[1] as f32 / u8::MAX as f32])
                        .collect(),
                    gltf::mesh::util::ReadTexCoords::U16(iter) => iter
                        .map(|uv| {
                            [
                                uv[0] as f32 / u16::MAX as f32,
                                uv[1] as f32 / u16::MAX as f32,
                            ]
                        })
                        .collect(),
                    gltf::mesh::util::ReadTexCoords::F32(iter) => iter.collect(),
                });
            let indices: Vec<u32> = reader
                .read_indices()
                .map(|i| i.into_u32().collect())
                .unwrap_or_else(|| (0..positions.len() as u32).collect());

            let vertices = positions
                .iter()
                .enumerate()
                .map(|(i, position)| {
                    let normal = normals
                        .as_ref()
                        .and_then(|list| list.get(i).copied())
                        .unwrap_or([0.0; 3]);
                    let uv = tex_coords
                        .as_ref()
                        .and_then(|list| list.get(i).copied())
                        .unwrap_or([0.0, 0.0]);
                    MeshVertex {
                        position: *position,
                        normal,
                        color: if normal == [0.0; 3] {
                            [0.75, 0.75, 0.8]
                        } else {
                            color_from_normal(normal)
                        },
                        uv,
                        bone_indices: [0; 4],
                        bone_weights: [1.0, 0.0, 0.0, 0.0],
                    }
                })
                .collect();

            let mesh = Mesh {
                name: mesh.name().unwrap_or(name).to_string(),
                vertices,
                indices,
                materials: Vec::new(),
                groups: Vec::new(),
                texture_paths: Vec::new(),
                bones: Vec::new(),
            };
            return Ok(mesh.finish_with_normals());
        }
    }

    Err(anyhow!("no triangle primitive found in {}", path.display()))
}

/// 按扩展名加载自定义模型：OBJ / glTF / GLB。
#[cfg(not(target_arch = "wasm32"))]
pub fn load_from_path(path: impl AsRef<std::path::Path>) -> Result<Mesh> {
    let path = path.as_ref();
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| anyhow!("model path has no extension: {}", path.display()))?;
    match extension.as_str() {
        "obj" => load_from_obj_path(path),
        "gltf" | "glb" => load_from_gltf_path(path),
        "pmx" => pmx::load_pmx_from_path(path),
        other => Err(anyhow!(
            "unsupported model format: .{other} (use obj/gltf/glb/pmx)"
        )),
    }
}
