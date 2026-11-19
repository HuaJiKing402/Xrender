struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct MaterialParams {
    diffuse: vec4<f32>,
};

@group(1) @binding(0) var<uniform> material: MaterialParams;
@group(1) @binding(1) var diffuse_texture: texture_2d<f32>;
@group(1) @binding(2) var diffuse_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = uniforms.mvp * vec4<f32>(input.position, 1.0);
    output.normal = normalize((uniforms.model * vec4<f32>(input.normal, 0.0)).xyz);
    output.color = input.color;
    output.uv = input.uv;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let light_direction = normalize(vec3<f32>(0.4, 0.8, 1.0));
    let diffuse = max(dot(normalize(input.normal), light_direction), 0.0);
    let shade = 0.28 + 0.72 * diffuse;
    let texel = textureSample(diffuse_texture, diffuse_sampler, input.uv);
    return vec4<f32>(input.color * material.diffuse.rgb * texel.rgb * shade, 1.0);
}
