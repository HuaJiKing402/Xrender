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
    if (index == INVALID_BONE_INDEX) {
        return identity_matrix();
    }
    return bone_matrices[index];
}

struct MaterialParams {
    diffuse: vec4<f32>,
    edge_color: vec4<f32>,
    params: vec4<f32>,
};

@group(1) @binding(0) var<uniform> material: MaterialParams;
@group(1) @binding(1) var diffuse_texture: texture_2d<f32>;
@group(1) @binding(2) var diffuse_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) bone_indices: vec4<u32>,
    @location(5) bone_weights: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
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
    var output: VertexOutput;
    output.position = uniforms.mvp * skinned;
    output.uv = input.uv;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let alpha = material.diffuse.a * textureSample(diffuse_texture, diffuse_sampler, input.uv).a;
    if (alpha < material.params.w) {
        discard;
    }
    return vec4<f32>(1.0);
}
