use glam::{Quat, Vec3};
use renderer_core::{
    model::{Bone, MaterialGroup, MaterialProfile, Mesh, MeshVertex},
    skinning::{compute_bone_matrices, Pose},
};

fn bone(position: [f32; 3]) -> Bone {
    Bone {
        name: "root".into(),
        parent: None,
        position,
    }
}

fn vertex(position: [f32; 3]) -> MeshVertex {
    MeshVertex {
        position,
        normal: [0.0, 0.0, 1.0],
        color: [1.0, 1.0, 1.0],
        uv: [0.0, 0.0],
        bone_indices: [0; 4],
        bone_weights: [1.0, 0.0, 0.0, 0.0],
    }
}

#[test]
fn rest_pose_keeps_vertex_in_place() {
    let bones = vec![bone([0.0, 10.0, 0.0])];
    let matrices = compute_bone_matrices(&bones, &Pose::default());
    let position = Vec3::new(1.0, 11.0, 2.0);
    let skinned = matrices[0].transform_point3(position);
    assert!((skinned - position).length() < 1e-5);
}

#[test]
fn bone_rotation_rotates_around_bone_origin() {
    let bones = vec![bone([0.0, 10.0, 0.0])];
    let mut pose = Pose::default();
    pose.set_rotation(0, Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    let matrices = compute_bone_matrices(&bones, &pose);
    let skinned = matrices[0].transform_point3(Vec3::new(1.0, 10.0, 0.0));
    assert!((skinned - Vec3::new(0.0, 10.0, -1.0)).length() < 1e-4);
}

#[test]
fn normalize_moves_bones_with_vertices() {
    let mut mesh = Mesh {
        name: "test".into(),
        vertices: vec![vertex([-1.0, 0.0, 0.0]), vertex([1.0, 2.0, 0.0])],
        indices: vec![0, 1, 0],
        materials: vec![MaterialProfile::opaque()],
        groups: vec![MaterialGroup {
            material_index: 0,
            index_start: 0,
            index_count: 3,
        }],
        texture_paths: Vec::new(),
        bones: vec![bone([0.0, 1.0, 0.0])],
    };
    let before = Vec3::from(mesh.vertices[0].position) - Vec3::from(mesh.bones[0].position);
    mesh.normalize();
    let after = Vec3::from(mesh.vertices[0].position) - Vec3::from(mesh.bones[0].position);
    let scale = 1.4 / 2.0;
    assert!((after - before * scale).length() < 1e-5);
}
