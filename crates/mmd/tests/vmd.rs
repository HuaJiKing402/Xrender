use std::iter;

use glam::Quat;
use mmd::vmd::parse_vmd;
use renderer_core::model::Bone;

fn padded(bytes: &[u8], len: usize) -> Vec<u8> {
    bytes
        .iter()
        .copied()
        .chain(iter::repeat_n(0, len.saturating_sub(bytes.len())))
        .take(len)
        .collect()
}

fn vmd_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(padded(b"Vocaloid Motion Data 0002", 30));
    out.extend(padded(b"test", 20));
    out.extend(3u32.to_le_bytes());

    for (frame, angle) in [(0u32, 0.0f32), (15, std::f32::consts::FRAC_PI_2), (30, 0.0)] {
        out.extend(padded(b"bone", 15));
        out.extend(frame.to_le_bytes());
        out.extend(0.0f32.to_le_bytes());
        out.extend(0.0f32.to_le_bytes());
        out.extend(0.0f32.to_le_bytes());
        let q = Quat::from_xyzw(0.0, (angle * 0.5).sin(), 0.0, (angle * 0.5).cos());
        for value in [q.x, q.y, q.z, q.w] {
            out.extend(value.to_le_bytes());
        }
        out.extend(iter::repeat_n(0, 64));
    }

    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out
}

#[test]
fn parses_vmd_and_samples_rotation() {
    let clip = parse_vmd(&vmd_bytes()).expect("VMD should parse");
    assert_eq!(clip.name, "test");
    assert_eq!(clip.bone_tracks["bone"].len(), 3);

    let bones = vec![Bone {
        name: "bone".into(),
        parent: None,
        position: [0.0; 3],
    }];

    let mid = clip.sample(7.5, &bones);
    let expected = Quat::from_xyzw(
        0.0,
        (std::f32::consts::FRAC_PI_8).sin(),
        0.0,
        (std::f32::consts::FRAC_PI_8).cos(),
    );
    let actual = mid.rotations[&0];
    assert!(actual.dot(expected).abs() > 0.999);

    let end = clip.sample(30.0, &bones);
    assert!(end.rotations[&0].dot(Quat::IDENTITY).abs() > 0.999);
}
