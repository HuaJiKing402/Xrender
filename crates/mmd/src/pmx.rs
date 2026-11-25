use anyhow::{bail, Context, Result};
use renderer_core::model::{
    texture::BUILTIN_RAMP_COUNT, Bone, MaterialGroup, MaterialProfile, Mesh, MeshVertex,
    OutlineStyle, RampSlot, ShadingData, SphereMap, SphereMode, ToonParams,
};

/// 把 PMX 的 toon / sphere 字段翻译成通用的卡通着色参数。
///
/// 这些是 MMD 自己的约定，所以放在装载器里而不是内核里：
/// - `toon_index < 0` 表示该材质根本没有 toon，回落成白色乘数（不压暗）。
/// - `toon_reference == 0` 时索引指向模型贴图表，有些模型直接拿漫反射图当 ramp。
/// - `toon_reference == 1` 时指向内置 toon01–10，越界时退到第一张。
/// - sphere 索引为负表示没有环境球；混合方式 1 乘算、2 加算，其余视为关闭。
pub fn toon_params(
    toon_reference: u8,
    toon_index: i32,
    environment_index: i64,
    sphere_mode: u8,
) -> ToonParams {
    let ramp = if toon_index < 0 {
        None
    } else if toon_reference == 0 {
        Some(RampSlot::Diffuse(toon_index as usize))
    } else {
        let index = (toon_index as usize).min(BUILTIN_RAMP_COUNT.saturating_sub(1));
        Some(RampSlot::Builtin(index))
    };
    let sphere = usize::try_from(environment_index).ok().and_then(|index| {
        let mode = match sphere_mode {
            1 => SphereMode::Multiply,
            2 => SphereMode::Add,
            _ => return None,
        };
        Some(SphereMap { index, mode })
    });
    ToonParams { ramp, sphere }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, len: usize, what: &str) -> Result<&'a [u8]> {
        if self.remaining() < len {
            bail!("unexpected EOF while reading {what} at offset {}", self.pos);
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn u8(&mut self, what: &str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    fn i8(&mut self, what: &str) -> Result<i8> {
        Ok(self.u8(what)? as i8)
    }

    fn u16(&mut self, what: &str) -> Result<u16> {
        let bytes = self.take(2, what)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn i16(&mut self, what: &str) -> Result<i16> {
        Ok(self.u16(what)? as i16)
    }

    fn u32(&mut self, what: &str) -> Result<u32> {
        let bytes = self.take(4, what)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i32(&mut self, what: &str) -> Result<i32> {
        Ok(self.u32(what)? as i32)
    }

    fn f32(&mut self, what: &str) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    fn vec3(&mut self, what: &str) -> Result<[f32; 3]> {
        Ok([self.f32(what)?, self.f32(what)?, self.f32(what)?])
    }

    fn vec4(&mut self, what: &str) -> Result<[f32; 4]> {
        Ok([
            self.f32(what)?,
            self.f32(what)?,
            self.f32(what)?,
            self.f32(what)?,
        ])
    }

    fn text(&mut self, encoding: u8) -> Result<String> {
        let length = self.i32("text length")?;
        if length < 0 {
            bail!("negative text length: {length}");
        }
        let bytes = self.take(length as usize, "text")?;
        match encoding {
            0 => {
                if bytes.len() % 2 != 0 {
                    bail!("UTF-16 text has odd byte length");
                }
                let units: Vec<u16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                String::from_utf16(&units).context("invalid UTF-16LE text in PMX")
            }
            1 => String::from_utf8(bytes.to_vec()).context("invalid UTF-8 text in PMX"),
            other => bail!("unsupported PMX text encoding: {other}"),
        }
    }

    fn unsigned_index(&mut self, size: u8, what: &str) -> Result<usize> {
        Ok(match size {
            1 => self.u8(what)? as usize,
            2 => self.u16(what)? as usize,
            4 => self.u32(what)? as usize,
            other => bail!("invalid index size {other} for {what}"),
        })
    }

    fn signed_index(&mut self, size: u8, what: &str) -> Result<i64> {
        Ok(match size {
            1 => self.i8(what)? as i64,
            2 => self.i16(what)? as i64,
            4 => self.i32(what)? as i64,
            other => bail!("invalid index size {other} for {what}"),
        })
    }
}

const INVALID_BONE_INDEX: u32 = u32::MAX;

fn read_bone_index(reader: &mut Reader<'_>, size: u8, what: &str) -> Result<Option<u32>> {
    let index = reader.signed_index(size, what)?;
    Ok(u32::try_from(index).ok())
}

/// 解析 PMX 2.0 / 2.1 的几何与材质颜色。
///
/// 骨骼、表情、刚体物理、贴图在此阶段不加载，后续版本迭代。
pub fn load_pmx(bytes: &[u8], name: &str) -> Result<Mesh> {
    let mut reader = Reader::new(bytes);

    let signature = reader.take(4, "signature")?;
    if signature != b"PMX " {
        bail!("not a PMX file: invalid signature");
    }
    let version = reader.f32("version")?;
    if !(2.0..=2.2).contains(&version) {
        bail!("unsupported PMX version: {version}");
    }

    let globals_count = reader.u8("globals count")? as usize;
    if globals_count < 8 {
        bail!("PMX globals count too small: {globals_count}");
    }
    let globals = reader.take(globals_count, "globals")?.to_vec();
    let encoding = globals[0];
    let additional_uv_count = globals[1] as usize;
    let vertex_index_size = globals[2];
    let texture_index_size = globals[3];
    let _material_index_size = globals[4];
    let bone_index_size = globals[5];
    let _morph_index_size = globals[6];
    let _rigidbody_index_size = globals[7];

    let name_local = reader.text(encoding)?;
    let name_universal = reader.text(encoding)?;
    let _comment_local = reader.text(encoding)?;
    let _comment_universal = reader.text(encoding)?;
    let model_name = if !name_universal.is_empty() {
        name_universal
    } else if !name_local.is_empty() {
        name_local
    } else {
        name.to_string()
    };

    let vertex_count = reader.i32("vertex count")?;
    if vertex_count < 0 {
        bail!("negative PMX vertex count");
    }
    let mut vertices = Vec::with_capacity(vertex_count as usize);
    for i in 0..vertex_count as usize {
        let position = reader.vec3("vertex position")?;
        let normal = reader.vec3("vertex normal")?;
        let uv_bytes = reader.take(8, "vertex uv")?;
        let uv = [
            f32::from_le_bytes([uv_bytes[0], uv_bytes[1], uv_bytes[2], uv_bytes[3]]),
            f32::from_le_bytes([uv_bytes[4], uv_bytes[5], uv_bytes[6], uv_bytes[7]]),
        ];
        reader.take(additional_uv_count * 16, "additional UV")?;
        let deform_type = reader.u8("deform type")?;
        let mut bone_indices = [0u32; 4];
        let mut bone_weights = [0.0f32; 4];
        match deform_type {
            0 => {
                let bone = read_bone_index(&mut reader, bone_index_size, "BDEF1 bone")?;
                bone_indices[0] = bone.unwrap_or(INVALID_BONE_INDEX);
                bone_weights[0] = if bone.is_some() { 1.0 } else { 0.0 };
            }
            1 => {
                let bone_0 = read_bone_index(&mut reader, bone_index_size, "BDEF2 bone 1")?;
                let bone_1 = read_bone_index(&mut reader, bone_index_size, "BDEF2 bone 2")?;
                let w = reader.f32("BDEF2 weight")?;
                bone_indices[0] = bone_0.unwrap_or(INVALID_BONE_INDEX);
                bone_indices[1] = bone_1.unwrap_or(INVALID_BONE_INDEX);
                bone_weights = [
                    if bone_0.is_some() { w } else { 0.0 },
                    if bone_1.is_some() { 1.0 - w } else { 0.0 },
                    0.0,
                    0.0,
                ];
            }
            2 | 4 => {
                for slot in 0..4 {
                    let bone = read_bone_index(&mut reader, bone_index_size, "BDEF4/QDEF bone")?;
                    bone_indices[slot] = bone.unwrap_or(INVALID_BONE_INDEX);
                    let weight = reader.f32("BDEF4/QDEF weight")?;
                    bone_weights[slot] = if bone.is_some() { weight } else { 0.0 };
                }
            }
            3 => {
                let bone_0 = read_bone_index(&mut reader, bone_index_size, "SDEF bone 1")?;
                let bone_1 = read_bone_index(&mut reader, bone_index_size, "SDEF bone 2")?;
                let w = reader.f32("SDEF weight")?;
                bone_indices[0] = bone_0.unwrap_or(INVALID_BONE_INDEX);
                bone_indices[1] = bone_1.unwrap_or(INVALID_BONE_INDEX);
                bone_weights = [
                    if bone_0.is_some() { w } else { 0.0 },
                    if bone_1.is_some() { 1.0 - w } else { 0.0 },
                    0.0,
                    0.0,
                ];
                reader.take(36, "SDEF C/R0/R1")?;
            }
            other => bail!("unsupported PMX deform type {other} at vertex {i}"),
        }
        let _edge_scale = reader.f32("edge scale")?;
        vertices.push(MeshVertex {
            position,
            normal,
            color: [1.0, 1.0, 1.0],
            uv,
            bone_indices,
            bone_weights,
        });
    }

    let index_count = reader.i32("surface count")?;
    if index_count < 0 || index_count % 3 != 0 {
        bail!("invalid PMX surface count: {index_count}");
    }
    let mut indices = Vec::with_capacity(index_count as usize);
    for _ in 0..index_count {
        let index = reader.unsigned_index(vertex_index_size, "surface index")?;
        if index >= vertices.len() {
            bail!("surface index {index} out of range");
        }
        indices.push(index as u32);
    }

    let texture_count = reader.i32("texture count")?;
    if texture_count < 0 {
        bail!("negative PMX texture count");
    }
    let mut texture_paths = Vec::with_capacity(texture_count.max(0) as usize);
    for _ in 0..texture_count {
        texture_paths.push(reader.text(encoding)?);
    }

    let material_count = reader.i32("material count")?;
    if material_count < 0 {
        bail!("negative PMX material count");
    }
    let mut materials = Vec::with_capacity(material_count as usize);
    let mut groups = Vec::with_capacity(material_count as usize);
    let mut surface_cursor = 0usize;
    for _ in 0..material_count {
        let name_local = reader.text(encoding)?;
        let name_universal = reader.text(encoding)?;
        let diffuse = reader.vec4("material diffuse")?;
        let specular = reader.vec3("material specular")?;
        let specular_strength = reader.f32("material specular strength")?;
        let ambient = reader.vec3("material ambient")?;
        let flags = reader.u8("material draw flags")?;
        let edge_color = reader.vec4("material edge color")?;
        let edge_scale = reader.f32("material edge scale")?;
        let texture_index = reader.signed_index(texture_index_size, "texture index")?;
        let environment_index = reader.signed_index(texture_index_size, "environment index")?;
        let sphere_mode = reader.u8("environment blend")?;
        let toon_reference = reader.u8("toon reference")?;
        let toon_index = match toon_reference {
            0 => reader.signed_index(texture_index_size, "toon texture index")? as i32,
            1 => reader.u8("internal toon index")? as i32,
            other => bail!("invalid toon reference: {other}"),
        };
        let _meta = reader.text(encoding)?;
        let surface_count = reader.i32("material surface count")?;
        if surface_count < 0 || surface_count % 3 != 0 {
            bail!("invalid material surface count: {surface_count}");
        }
        let surface_count = surface_count as usize;
        if surface_cursor + surface_count > indices.len() {
            bail!("material surface range exceeds surface list");
        }

        let material_index = materials.len();
        materials.push(MaterialProfile {
            name: if !name_universal.is_empty() {
                name_universal
            } else {
                name_local
            },
            diffuse,
            ambient,
            specular,
            specular_strength,
            texture_index: usize::try_from(texture_index).ok(),
            // PMX 的描边开关是 flags & 0x10；其余标志位（双面等）暂未使用。
            outline: (flags & 0x10 != 0).then_some(OutlineStyle {
                color: edge_color,
                scale: edge_scale,
            }),
            shading: ShadingData::Toon(toon_params(
                toon_reference,
                toon_index,
                environment_index,
                sphere_mode,
            )),
        });
        groups.push(MaterialGroup {
            material_index,
            index_start: surface_cursor,
            index_count: surface_count,
        });
        surface_cursor += surface_count;
    }

    let bone_count = reader.i32("bone count")?;
    if bone_count < 0 {
        bail!("negative PMX bone count");
    }
    let mut bones = Vec::with_capacity(bone_count as usize);
    for _ in 0..bone_count {
        let name_local = reader.text(encoding)?;
        let name_universal = reader.text(encoding)?;
        let position = reader.vec3("bone position")?;
        let parent_raw = reader.signed_index(bone_index_size, "parent bone")?;
        let layer = reader.i32("bone layer")?;
        let flags = reader.u16("bone flags")?;

        if flags & 0x0001 != 0 {
            reader.signed_index(bone_index_size, "tail bone")?;
        } else {
            reader.vec3("tail position")?;
        }
        if flags & (0x0100 | 0x0200) != 0 {
            reader.signed_index(bone_index_size, "append parent")?;
            reader.f32("append rate")?;
        }
        if flags & 0x0400 != 0 {
            reader.vec3("fixed axis")?;
        }
        if flags & 0x0800 != 0 {
            reader.vec3("local x axis")?;
            reader.vec3("local z axis")?;
        }
        if flags & 0x2000 != 0 {
            reader.i32("external parent key")?;
        }
        if flags & 0x0020 != 0 {
            reader.signed_index(bone_index_size, "IK target")?;
            reader.i32("IK iterations")?;
            reader.f32("IK limit angle")?;
            let link_count = reader.i32("IK link count")?;
            if link_count < 0 {
                bail!("negative IK link count");
            }
            for _ in 0..link_count {
                reader.signed_index(bone_index_size, "IK link bone")?;
                if reader.u8("IK angle limit flag")? != 0 {
                    reader.vec3("IK lower limit")?;
                    reader.vec3("IK upper limit")?;
                }
            }
        }

        bones.push(Bone {
            name: if !name_universal.is_empty() {
                name_universal
            } else {
                name_local
            },
            parent: usize::try_from(parent_raw).ok(),
            position,
            layer,
            flags,
        });
    }

    // 顶点块在骨骼块之前解析，越界的正索引到这里才知道。
    // 统一改成 INVALID_BONE_INDEX，shader 会把它当 identity 矩阵，
    // 避免越界读取零矩阵后把法线退化成 NaN。
    let bone_count_u32 = bones.len() as u32;
    for vertex in &mut vertices {
        for index in &mut vertex.bone_indices {
            if *index != INVALID_BONE_INDEX && *index >= bone_count_u32 {
                *index = INVALID_BONE_INDEX;
            }
        }
    }

    let mesh = Mesh {
        name: model_name,
        vertices,
        indices,
        materials,
        groups,
        texture_paths,
        bones,
    };
    Ok(mesh.finish())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_pmx_from_path(path: impl AsRef<std::path::Path>) -> Result<Mesh> {
    let path = path.as_ref();
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let fallback = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("pmx-model");
    load_pmx(&bytes, fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_bone_index_is_none() {
        let bytes = [0xff];
        let mut reader = Reader::new(&bytes);
        assert_eq!(
            read_bone_index(&mut reader, 1, "bone").expect("index"),
            None
        );
    }
}
