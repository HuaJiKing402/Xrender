use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};

use crate::model::Bone;

/// 骨骼姿态：只保存相对 rest pose 的旋转/平移覆盖。
#[derive(Clone, Debug, Default)]
pub struct Pose {
    pub rotations: HashMap<usize, Quat>,
    pub translations: HashMap<usize, Vec3>,
}

impl Pose {
    pub fn set_rotation(&mut self, bone: usize, rotation: Quat) {
        self.rotations.insert(bone, rotation);
    }

    pub fn set_translation(&mut self, bone: usize, translation: Vec3) {
        self.translations.insert(bone, translation);
    }
}

/// 根据骨骼层级与姿态计算每根骨骼的全局矩阵。
pub fn compute_bone_matrices(bones: &[Bone], pose: &Pose) -> Vec<Mat4> {
    let mut globals = vec![Mat4::IDENTITY; bones.len()];
    for (index, bone) in bones.iter().enumerate() {
        let parent_position = bone
            .parent
            .and_then(|parent| bones.get(parent))
            .map(|parent| Vec3::from(parent.position))
            .unwrap_or(Vec3::ZERO);
        let local_translation = Vec3::from(bone.position) - parent_position
            + pose.translations.get(&index).copied().unwrap_or(Vec3::ZERO);
        let rotation = pose
            .rotations
            .get(&index)
            .copied()
            .unwrap_or(Quat::IDENTITY);
        let local = Mat4::from_translation(local_translation) * Mat4::from_quat(rotation);
        globals[index] = bone
            .parent
            .and_then(|parent| globals.get(parent).copied())
            .map(|parent_global| parent_global * local)
            .unwrap_or(local);
    }
    globals
}
