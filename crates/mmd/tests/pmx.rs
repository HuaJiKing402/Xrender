use mmd::pmx::{load_pmx, toon_params};
use renderer_core::model::{RampSlot, ShadingData, SphereMode};

#[test]
fn parses_pmx_geometry_and_material_color() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../models/triangle.pmx"
    ))
    .expect("sample PMX should exist");
    let mesh = load_pmx(&bytes, "fallback").expect("PMX should parse");
    assert_eq!(mesh.name, "CrossRender PMX Triangle");
    assert_eq!(mesh.vertex_count(), 3);
    assert_eq!(mesh.triangle_count(), 1);
    assert_eq!(mesh.indices, vec![0, 1, 2]);
    assert!(mesh.vertices[0].color[0] > 0.9);
    // 材质的 toon 参数走通用的卡通着色扩展槽，不再是内核里的 MMD 专有结构。
    assert!(matches!(mesh.materials[0].shading, ShadingData::Toon(_)));
}

#[test]
fn negative_toon_index_means_no_ramp() {
    // PMX 里 toon 索引为 -1 = 该材质没有 toon；必须保留白色乘数，
    // 不能悄悄退回 toon01，否则无 toon 材质会凭空多一层灰影。
    let params = toon_params(0, -1, -1, 0);
    assert!(params.ramp.is_none());
    assert!(params.sphere.is_none());
}

#[test]
fn toon_reference_selects_which_table() {
    // 0 = 模型贴图表（有些模型直接拿漫反射图当 ramp），1 = 内置 toon01–10。
    let diffuse = toon_params(0, 3, -1, 0);
    assert_eq!(diffuse.ramp, Some(RampSlot::Diffuse(3)));

    let builtin = toon_params(1, 6, -1, 0);
    assert_eq!(builtin.ramp, Some(RampSlot::Builtin(6)));
}

#[test]
fn builtin_toon_index_is_clamped_to_the_table() {
    // 内置表只有 10 张；越界退到最后一张，避免整片材质失去阴影层次。
    let params = toon_params(1, 99, -1, 0);
    assert_eq!(params.ramp, Some(RampSlot::Builtin(9)));
}

#[test]
fn sphere_mode_maps_to_multiply_or_add() {
    let multiply = toon_params(1, 0, 0, 1);
    assert_eq!(
        multiply.sphere.map(|sphere| sphere.mode),
        Some(SphereMode::Multiply)
    );

    let add = toon_params(1, 0, 2, 2);
    assert_eq!(add.sphere.map(|sphere| sphere.mode), Some(SphereMode::Add));

    // 混合方式 0（关闭）与非法值都不启用环境球。
    assert!(toon_params(1, 0, 0, 0).sphere.is_none());
    assert!(toon_params(1, 0, 0, 3).sphere.is_none());
}
