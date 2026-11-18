use renderer_core::model;

#[test]
fn parses_obj_triangles_and_quads() {
    let source = r#"
        v 0 0 0
        v 1 0 0
        v 1 1 0
        v 0 1 0
        vn 0 0 1
        f 1//1 2//1 3//1 4//1
    "#;
    let mesh = model::load_obj(source, "test").expect("OBJ should parse");
    assert_eq!(mesh.vertex_count(), 4);
    assert_eq!(mesh.triangle_count(), 2);
    assert!(mesh.vertices.iter().all(|v| v.normal != [0.0; 3]));
}

#[test]
fn builtin_triangle_is_usable() {
    let mesh = model::Mesh::triangle();
    assert_eq!(mesh.triangle_count(), 1);
    assert_eq!(mesh.indices, vec![0, 1, 2]);
}
