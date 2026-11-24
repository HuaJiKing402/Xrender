use mmd::pmx::load_pmx;

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
}
