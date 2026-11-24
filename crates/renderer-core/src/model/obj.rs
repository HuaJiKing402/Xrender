use anyhow::{anyhow, Context, Result};

use super::{color_from_normal, Mesh, MeshVertex};

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
pub(crate) fn load_from_path(path: &std::path::Path) -> Result<Mesh> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("obj-model");
    load_obj(&source, name)
}
