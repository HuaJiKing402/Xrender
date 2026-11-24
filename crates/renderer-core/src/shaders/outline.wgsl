struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var<storage, read> bone_matrices: array<mat4x4<f32>>;

const INVALID_BONE_INDEX: u32 = 4294967295u;

fn identity_matrix() -> mat4x4<f32> {
    return mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
}

fn bone_matrix(index: u32) -> mat4x4<f32> {
    if (index == INVALID_BONE_INDEX || index >= arrayLength(&bone_matrices)) {
        return identity_matrix();
    }
    return bone_matrices[index];
}

fn safe_normalize(v: vec3<f32>) -> vec3<f32> {
    let len = length(v);
    if (len < 1.0e-6) {
        return vec3<f32>(0.0, 0.0, 1.0);
    }
    return v / len;
}

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
    let total = input.bone_weights.x + input.bone_weights.y + input.bone_weights.z + input.bone_weights.w;
    var skin = identity_matrix();
    if (total > 0.0001) {
        let w = input.bone_weights / total;
        skin = bone_matrix(input.bone_indices.x) * w.x
            + bone_matrix(input.bone_indices.y) * w.y
            + bone_matrix(input.bone_indices.z) * w.z
            + bone_matrix(input.bone_indices.w) * w.w;
    }
    let skinned = skin * vec4<f32>(input.position, 1.0);
    let skin_normal = safe_normalize(mat3x3<f32>(skin[0].xyz, skin[1].xyz, skin[2].xyz) * input.normal);
    // params.x 是 CPU 端算好的绝对膨胀量（edge_scale × 模型尺寸比例），
    // shader 不再乘写死的常数，方便以后按模型尺寸或视距调整。
    let expanded = skinned.xyz + skin_normal * material.params.x;
    return uniforms.mvp * vec4<f32>(expanded, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return material.edge_color;
}
