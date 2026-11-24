struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    camera_position: vec4<f32>,
    view_rotation: mat4x4<f32>,
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
    ambient: vec4<f32>,
    specular: vec4<f32>,
};

@group(1) @binding(0) var<uniform> material: MaterialParams;
@group(1) @binding(1) var diffuse_texture: texture_2d<f32>;
@group(1) @binding(2) var diffuse_sampler: sampler;
@group(1) @binding(3) var toon_texture: texture_2d<f32>;
@group(1) @binding(4) var sphere_texture: texture_2d<f32>;
// toon 图与 sphere 图都必须 Clamp：它们是查找表/环境球，不是可平铺纹理。
@group(1) @binding(5) var ramp_sampler: sampler;

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
    // toon 图是明暗查找表：亮部是白色（×1 保持原色），暗部是影色（直接乘上去），
    // 色阶数量决定阴影是硬卡通还是渐变。用 Clamp 采样，避免两端颜色互相串。
    let toon = textureSample(toon_texture, ramp_sampler, toon_uv).rgb;
    let texel = textureSample(diffuse_texture, diffuse_sampler, input.uv);
    let alpha = material.diffuse.a * texel.a;
    if (alpha < material.params.w) {
        discard;
    }

    // ramp 直接当乘数：亮部白色 → 原色，暗部影色 → 乘上影色。
    // 官方 toon01 的影色是 205 灰左右，乘上去就是 MMD 的硬边阴影；
    // 这个模型所有材质的 ambient 都是常量 0.5，再叠一层只会把阴影整体抬亮，
    // 所以 ambient 不再参与 toon 明暗（阴影色完全由 ramp 决定）。
    let base = input.color * material.diffuse.rgb * texel.rgb;
    var color = base * toon;
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
        // MMD 的 sphere map 约定在相机空间：先把法线转到视图空间，
        // 再用视线方向求反射。直接拿世界空间反射向量当 UV，模型一转取样区就跑偏。
        let normal_view = normalize((uniforms.view_rotation * vec4<f32>(normal, 0.0)).xyz);
        let reflection = reflect(vec3<f32>(0.0, 0.0, -1.0), normal_view);
        // 视图空间 +Y 向上，而 sphere 图的 v=0 在顶部，所以 y 取反。
        let sphere_uv = vec2<f32>(reflection.x * 0.5 + 0.5, 0.5 - reflection.y * 0.5);
        let sphere = textureSample(sphere_texture, ramp_sampler, sphere_uv).rgb;
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
