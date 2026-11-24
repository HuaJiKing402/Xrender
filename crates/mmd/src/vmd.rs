//! VMD（Vocaloid Motion Data）解析：只负责字节 → `AnimationClip`。
//!
//! 采样、合并、应用姿态都在 `renderer_core::animation` 里，内核不认识 VMD。

use std::collections::HashMap;

use anyhow::{bail, Result};
use glam::{Quat, Vec3};
use renderer_core::animation::{AnimationClip, BoneKeyframe, MorphKeyframe};

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, len: usize, what: &str) -> Result<&'a [u8]> {
        if self.data.len().saturating_sub(self.pos) < len {
            bail!("unexpected EOF while reading {what} at offset {}", self.pos);
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn u32(&mut self, what: &str) -> Result<u32> {
        let bytes = self.take(4, what)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn f32(&mut self, what: &str) -> Result<f32> {
        let bytes = self.take(4, what)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn vec3(&mut self, what: &str) -> Result<Vec3> {
        Ok(Vec3::new(self.f32(what)?, self.f32(what)?, self.f32(what)?))
    }

    fn text(&mut self, len: usize, what: &str) -> Result<String> {
        let bytes = self.take(len, what)?;
        let (text, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
        Ok(text.trim_end_matches('\0').to_string())
    }
}

/// 解析 VMD 运动文件的骨骼/表情轨道；相机/灯光帧仅跳过。
pub fn parse_vmd(bytes: &[u8]) -> Result<AnimationClip> {
    let mut cursor = Cursor::new(bytes);
    let signature = cursor.take(30, "VMD signature")?;
    if !signature.starts_with(b"Vocaloid Motion Data") {
        bail!("not a VMD file: invalid signature");
    }
    let name = cursor.text(20, "VMD model name")?;

    let bone_count = cursor.u32("bone frame count")?;
    let mut bone_tracks: HashMap<String, Vec<BoneKeyframe>> = HashMap::new();
    for _ in 0..bone_count {
        let bone_name = cursor.text(15, "bone name")?;
        let frame = cursor.u32("bone frame")?;
        let position = cursor.vec3("bone position")?;
        let x = cursor.f32("bone quaternion x")?;
        let y = cursor.f32("bone quaternion y")?;
        let z = cursor.f32("bone quaternion z")?;
        let w = cursor.f32("bone quaternion w")?;
        cursor.take(64, "bone interpolation")?;
        let rotation = Quat::from_xyzw(x, y, z, w);
        bone_tracks
            .entry(bone_name)
            .or_default()
            .push(BoneKeyframe {
                frame,
                position,
                rotation,
            });
    }

    let morph_count = cursor.u32("morph frame count")?;
    let mut morph_tracks: HashMap<String, Vec<MorphKeyframe>> = HashMap::new();
    for _ in 0..morph_count {
        let morph_name = cursor.text(15, "morph name")?;
        let frame = cursor.u32("morph frame")?;
        let weight = cursor.f32("morph weight")?;
        morph_tracks
            .entry(morph_name)
            .or_default()
            .push(MorphKeyframe { frame, weight });
    }

    let camera_count = cursor.u32("camera frame count")?;
    cursor.take(61 * camera_count as usize, "camera frames")?;
    let light_count = cursor.u32("light frame count")?;
    cursor.take(28 * light_count as usize, "light frames")?;

    Ok(AnimationClip {
        bone_tracks,
        morph_tracks,
        name,
    })
}
