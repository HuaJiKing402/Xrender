use std::collections::HashMap;

use anyhow::{bail, Result};
use glam::{Quat, Vec3};

use crate::{model::Bone, skinning::Pose};

#[derive(Clone, Debug)]
pub struct BoneKeyframe {
    pub frame: u32,
    pub position: Vec3,
    pub rotation: Quat,
}

#[derive(Clone, Debug)]
pub struct MorphKeyframe {
    pub frame: u32,
    pub weight: f32,
}

#[derive(Clone, Debug, Default)]
pub struct VmdClip {
    pub bone_tracks: HashMap<String, Vec<BoneKeyframe>>,
    pub morph_tracks: HashMap<String, Vec<MorphKeyframe>>,
    pub model_name: String,
}

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

impl VmdClip {
    /// 解析 VMD 运动文件的骨骼/表情轨道；相机/灯光帧仅跳过。
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(bytes);
        let signature = cursor.take(30, "VMD signature")?;
        if !signature.starts_with(b"Vocaloid Motion Data") {
            bail!("not a VMD file: invalid signature");
        }
        let model_name = cursor.text(20, "VMD model name")?;

        let bone_count = cursor.u32("bone frame count")?;
        let mut bone_tracks: HashMap<String, Vec<BoneKeyframe>> = HashMap::new();
        for _ in 0..bone_count {
            let name = cursor.text(15, "bone name")?;
            let frame = cursor.u32("bone frame")?;
            let position = cursor.vec3("bone position")?;
            let x = cursor.f32("bone quaternion x")?;
            let y = cursor.f32("bone quaternion y")?;
            let z = cursor.f32("bone quaternion z")?;
            let w = cursor.f32("bone quaternion w")?;
            cursor.take(64, "bone interpolation")?;
            let rotation = Quat::from_xyzw(x, y, z, w);
            bone_tracks.entry(name).or_default().push(BoneKeyframe {
                frame,
                position,
                rotation,
            });
        }

        let morph_count = cursor.u32("morph frame count")?;
        let mut morph_tracks: HashMap<String, Vec<MorphKeyframe>> = HashMap::new();
        for _ in 0..morph_count {
            let name = cursor.text(15, "morph name")?;
            let frame = cursor.u32("morph frame")?;
            let weight = cursor.f32("morph weight")?;
            morph_tracks
                .entry(name)
                .or_default()
                .push(MorphKeyframe { frame, weight });
        }

        let camera_count = cursor.u32("camera frame count")?;
        cursor.take(61 * camera_count as usize, "camera frames")?;
        let light_count = cursor.u32("light frame count")?;
        cursor.take(28 * light_count as usize, "light frames")?;

        Ok(Self {
            bone_tracks,
            morph_tracks,
            model_name,
        })
    }

    /// 按帧号采样骨骼姿态。VMD 的骨骼 position 是相对 rest 的局部增量，
    /// 直接写入 Pose，不能再减去 PMX 骨骼位置。
    pub fn sample(&self, frame: f32, bones: &[Bone]) -> Pose {
        let mut pose = Pose::default();
        for (index, bone) in bones.iter().enumerate() {
            let Some(track) = self.bone_tracks.get(&bone.name) else {
                continue;
            };
            if track.is_empty() {
                continue;
            }
            let sampled = sample_track(track, frame);
            pose.set_rotation(index, sampled.rotation);
            pose.set_translation(index, sampled.position);
        }
        pose
    }
}

fn sample_track(track: &[BoneKeyframe], frame: f32) -> BoneKeyframe {
    if track.len() == 1 || frame <= track[0].frame as f32 {
        return track[0].clone();
    }
    let last = track.last().expect("non-empty track");
    if frame >= last.frame as f32 {
        return last.clone();
    }

    for pair in track.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let start = a.frame as f32;
        let end = b.frame as f32;
        if frame >= start && frame <= end {
            let t = ((frame - start) / (end - start).max(f32::EPSILON)).clamp(0.0, 1.0);
            return BoneKeyframe {
                frame: frame as u32,
                position: a.position.lerp(b.position, t),
                rotation: Quat::slerp(a.rotation, b.rotation, t),
            };
        }
    }
    last.clone()
}
