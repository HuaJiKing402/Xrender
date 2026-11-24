struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    camera_position: vec4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var<storage, read> bone_matrices: array<mat4x4<f32>>;

struct MaterialParams {
    diffuse: vec4<f32>,
    edge_color: vec4<f32>,
    params: vec4<f32>,
    ambient: vec4<f32>,
    specular: vec4<f32>,
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
    @location(4) bone_indices: vec4<u32>,
    @location(5) bone_weights: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) world_position: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let total = max(input.bone_weights.x + input.bone_weights.y + input.bone_weights.z + input.bone_weights.w, 0.00001);
    let w = input.bone_weights / total;
    let skin = bone_matrices[input.bone_indices.x] * w.x
        + bone_matrices[input.bone_indices.y] * w.y
        + bone_matrices[input.bone_indices.z] * w.z
        + bone_matrices[input.bone_indices.w] * w.w;
    let skinned = skin * vec4<f32>(input.position, 1.0);
    let skin_normal = mat3x3<f32>(skin[0].xyz, skin[1].xyz, skin[2].xyz) * input.normal;

    var output: VertexOutput;
    output.clip_position = uniforms.mvp * skinned;
    output.normal = normalize((uniforms.model * vec4<f32>(skin_normal, 0.0)).xyz);
    output.color = input.color;
    output.uv = input.uv;
    output.world_position = (uniforms.model * skinned).xyz;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let normal = normalize(input.normal);
    let light_direction = normalize(vec3<f32>(0.4, 0.8, 1.0));
    let view_direction = normalize(uniforms.camera_position.xyz - input.world_position);
    // 半兰伯特：把 dot 从 [-1,1] 映射到 [0,1]，让掠射面仍落在线性渐变里，
    // 而不是背光侧一律 clamp 到 0（那样整片背光会塌成同一个色阶）。
    let shade = clamp(dot(normal, light_direction) * 0.5 + 0.5, 0.0, 1.0);
    // toon 图的渐变轴与亮端位置由 CPU 端检测后编码进 ambient.w，
    // 不能一律按 (shade, 0.5) 采样，否则纵向 toon 图会被压成一条线。
    let toon_code = u32(material.ambient.w + 0.5);
    var toon_uv = vec2<f32>(shade, 0.5);
    if (toon_code == 1u) {
        toon_uv = vec2<f32>(0.5, shade);
    } else if (toon_code == 2u) {
        toon_uv = vec2<f32>(0.5, 1.0 - shade);
    } else if (toon_code == 3u) {
        toon_uv = vec2<f32>(1.0 - shade, 0.5);
    }
    let toon = textureSample(toon_texture, diffuse_sampler, toon_uv).rgb;
    let texel = textureSample(diffuse_texture, diffuse_sampler, input.uv);
    let alpha = material.diffuse.a * texel.a;
    if (alpha < material.params.w) {
        discard;
    }

    // MMD 材质：ambient 是"暗部地板"，而不是和 toon 一起叠乘的增益。
    // 旧写法 base*(ambient+toon) 会把 ambient=0.5 的材质亮面推到 1.5× 过曝。
    let ambient = clamp(material.ambient.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let base = input.color * material.diffuse.rgb * texel.rgb;
    var color = base * (ambient + toon * (vec3<f32>(1.0) - ambient));
    // specular_strength 在 PMX 里是高光指数；0 表示关闭高光。
    // 直接 clamp 到 1 会把"关闭"变成一层铺满整个表面的宽高光，所以先门控。
    if (material.specular.w > 0.0) {
        let half_direction = normalize(light_direction + view_direction);
        let specular_exponent = clamp(material.specular.w, 1.0, 100.0);
        let highlight = pow(max(dot(normal, half_direction), 0.0), specular_exponent);
        color += material.specular.rgb * highlight;
    }

    let sphere_mode = u32(material.params.y);
    let sphere_enabled = material.params.z > 0.5;
    if (sphere_enabled && (sphere_mode == 1u || sphere_mode == 2u)) {
        // sphere map 使用反射向量，而不是直接拿法线当 UV。
        let reflection = reflect(-view_direction, normal);
        let sphere_uv = vec2<f32>(reflection.x * 0.5 + 0.5, reflection.y * 0.5 + 0.5);
        let sphere = textureSample(sphere_texture, diffuse_sampler, sphere_uv).rgb;
        if (sphere_mode == 1u) {
            color = color * sphere;
        } else {
            color = color + sphere;
        }
    }
    // 高光与 sphere 加算都可能把颜色推出 [0,1]，统一在出口钳制。
    color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(color, alpha);
}
