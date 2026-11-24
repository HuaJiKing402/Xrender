//! 与格式无关的动画数据：按骨骼名组织的轨道 + 采样。
//!
//! 具体文件格式（VMD 等）的字节解析放在对应的资源 crate 里，
//! 内核只消费这里的 `AnimationClip`，因此换动作格式不需要改渲染层。

use std::collections::HashMap;

use glam::{Quat, Vec3};

use crate::{model::Bone, skinning::Pose};

/// 单个通道的三次贝塞尔缓动。控制点使用 0..1 归一化坐标，
/// 起点固定为 (0, 0)，终点固定为 (1, 1)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BezierCurve {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl Default for BezierCurve {
    fn default() -> Self {
        // MMD 默认插值 20,20,107,107（归一化后）。
        Self {
            x1: 20.0 / 127.0,
            y1: 20.0 / 127.0,
            x2: 107.0 / 127.0,
            y2: 107.0 / 127.0,
        }
    }
}

impl BezierCurve {
    pub fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self {
            x1: x1.clamp(0.0, 1.0),
            y1: y1.clamp(0.0, 1.0),
            x2: x2.clamp(0.0, 1.0),
            y2: y2.clamp(0.0, 1.0),
        }
    }

    /// 输入时间 t，返回缓动后的值。
    pub fn ease(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }

        // 先解 x(s) = t，再用同一个 s 求 y(s)。Newton 足够，
        // 最后用 clamp 保证极端控制点也稳定。
        let mut s = t;
        for _ in 0..8 {
            let x = cubic(0.0, self.x1, self.x2, 1.0, s);
            let dx = cubic_derivative(0.0, self.x1, self.x2, 1.0, s);
            if dx.abs() < 1.0e-5 {
                break;
            }
            s = (s - (x - t) / dx).clamp(0.0, 1.0);
        }
        cubic(0.0, self.y1, self.y2, 1.0, s).clamp(0.0, 1.0)
    }
}

/// 一个骨骼关键帧的四通道插值曲线，对应 VMD 的 64 字节。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneInterpolation {
    pub x: BezierCurve,
    pub y: BezierCurve,
    pub z: BezierCurve,
    pub rotation: BezierCurve,
}

impl Default for BoneInterpolation {
    fn default() -> Self {
        Self {
            x: BezierCurve::default(),
            y: BezierCurve::default(),
            z: BezierCurve::default(),
            rotation: BezierCurve::default(),
        }
    }
}

fn cubic(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let mt = 1.0 - t;
    mt * mt * mt * p0 + 3.0 * mt * mt * t * p1 + 3.0 * mt * t * t * p2 + t * t * t * p3
}

fn cubic_derivative(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let mt = 1.0 - t;
    3.0 * mt * mt * (p1 - p0) + 6.0 * mt * t * (p2 - p1) + 3.0 * t * t * (p3 - p2)
}

#[derive(Clone, Debug)]
pub struct BoneKeyframe {
    pub frame: u32,
    pub position: Vec3,
    pub rotation: Quat,
    pub interpolation: BoneInterpolation,
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
            // VMD 的插值曲线属于目的关键帧，即描述 a -> b 这一段。
            let x = b.interpolation.x.ease(t);
            let y = b.interpolation.y.ease(t);
            let z = b.interpolation.z.ease(t);
            let rotation_t = b.interpolation.rotation.ease(t);
            return BoneKeyframe {
                frame: frame as u32,
                position: Vec3::new(
                    a.position.x + (b.position.x - a.position.x) * x,
                    a.position.y + (b.position.y - a.position.y) * y,
                    a.position.z + (b.position.z - a.position.z) * z,
                ),
                rotation: Quat::slerp(a.rotation, b.rotation, rotation_t),
                interpolation: b.interpolation,
            };
        }
    }
    last.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bezier_is_linear() {
        let curve = BezierCurve::default();
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!((curve.ease(t) - t).abs() < 1.0e-3, "t={t}");
        }
    }

    #[test]
    fn bezier_supports_ease_in_and_out() {
        let ease_in = BezierCurve::new(0.8, 0.0, 0.8, 0.0);
        let ease_out = BezierCurve::new(0.0, 0.8, 0.0, 0.8);
        assert!(ease_in.ease(0.5) < 0.5);
        assert!(ease_out.ease(0.5) > 0.5);
    }

    #[test]
    fn destination_keyframe_owns_the_segment_curve() {
        let key = |frame, position| BoneKeyframe {
            frame,
            position,
            rotation: Quat::IDENTITY,
            interpolation: BoneInterpolation::default(),
        };
        let a = key(0, Vec3::ZERO);
        let mut b = key(10, Vec3::new(10.0, 0.0, 0.0));
        b.interpolation.x = BezierCurve::new(0.8, 0.0, 0.8, 0.0);

        let sampled = sample_track(&[a, b], 5.0);
        assert!(sampled.position.x < 5.0);
    }
}
