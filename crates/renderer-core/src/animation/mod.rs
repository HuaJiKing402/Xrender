//! 与格式无关的动画数据：按骨骼名组织的轨道 + 采样。
//!
//! 具体文件格式（VMD 等）的字节解析放在对应的资源 crate 里，
//! 内核只消费这里的 `AnimationClip`，因此换动作格式不需要改渲染层。

use std::collections::HashMap;

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

/// 一段动作：骨骼轨道与表情轨道，按骨骼/表情名索引。
#[derive(Clone, Debug, Default)]
pub struct AnimationClip {
    pub bone_tracks: HashMap<String, Vec<BoneKeyframe>>,
    pub morph_tracks: HashMap<String, Vec<MorphKeyframe>>,
    pub name: String,
}

impl AnimationClip {
    /// 按帧号采样骨骼姿态。轨道里的 position 是相对 rest 的局部增量，
    /// 直接写入 Pose，不再减去骨骼位置。
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
