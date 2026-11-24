use anyhow::{anyhow, Context, Result};

use super::{color_from_normal, Mesh, MeshVertex};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_from_path(path: &std::path::Path) -> Result<Mesh> {
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
