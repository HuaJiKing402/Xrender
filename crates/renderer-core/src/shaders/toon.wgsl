struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct MaterialParams {
    diffuse: vec4<f32>,
    edge_color: vec4<f32>,
    params: vec4<f32>,
};

@group(1) @binding(0) var<uniform> material: MaterialParams;
@group(1) @binding(1) var diffuse_texture: texture_2d<f32>;
@group(1) @binding(2) var diffuse_sampler: sampler;
@group(1) @binding(3) var toon_texture: texture_2d<f32>;
@group(1) @binding(4) var sphere_texture: texture_2d<f32>;

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
    let normal = normalize(input.normal);
    let light_direction = normalize(vec3<f32>(0.4, 0.8, 1.0));
    let ndl = clamp(dot(normal, light_direction), 0.0, 1.0);
    let toon = textureSample(toon_texture, diffuse_sampler, vec2<f32>(ndl, 0.5)).rgb;
    let texel = textureSample(diffuse_texture, diffuse_sampler, input.uv);
    var color = input.color * material.diffuse.rgb * texel.rgb * toon;

    let sphere_mode = u32(material.params.y);
    if (sphere_mode == 1u || sphere_mode == 2u) {
        let sphere_uv = vec2<f32>(normal.x * 0.5 + 0.5, normal.y * 0.5 + 0.5);
        let sphere = textureSample(sphere_texture, diffuse_sampler, sphere_uv).rgb;
        if (sphere_mode == 1u) {
            color = color * sphere;
        } else {
            color = color + sphere;
        }
    }
    return vec4<f32>(color, 1.0);
}
