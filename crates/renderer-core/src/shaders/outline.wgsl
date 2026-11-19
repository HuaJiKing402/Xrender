struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var<storage, read> bone_matrices: array<mat4x4<f32>>;

struct MaterialParams {
    diffuse: vec4<f32>,
    edge_color: vec4<f32>,
    params: vec4<f32>,
};

@group(1) @binding(0) var<uniform> material: MaterialParams;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) bone_indices: vec4<u32>,
    @location(5) bone_weights: vec4<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> @builtin(position) vec4<f32> {
    let total = max(input.bone_weights.x + input.bone_weights.y + input.bone_weights.z + input.bone_weights.w, 0.00001);
    let w = input.bone_weights / total;
    let skin = bone_matrices[input.bone_indices.x] * w.x
        + bone_matrices[input.bone_indices.y] * w.y
        + bone_matrices[input.bone_indices.z] * w.z
        + bone_matrices[input.bone_indices.w] * w.w;
    let skinned = skin * vec4<f32>(input.position, 1.0);
    let skin_normal = normalize(mat3x3<f32>(skin[0].xyz, skin[1].xyz, skin[2].xyz) * input.normal);
    let expanded = skinned.xyz + skin_normal * material.params.x * 0.012;
    return uniforms.mvp * vec4<f32>(expanded, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return material.edge_color;
}
