use std::{path::Path, time::Instant};

use anyhow::{anyhow, Result};
use glam::{Mat3, Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::{
    animation::AnimationClip,
    backend::BACKEND_LABEL,
    camera::OrbitCamera,
    capture::FrameImage,
    gpu::GpuContext,
    material::create_material_state,
    model::{
        texture::{self, TextureSlot},
        Bone, Mesh,
    },
    overlay::FpsOverlay,
    pass::{PassDraw, ScenePasses},
    pipeline::PipelineCache,
    renderer_helpers::{
        blit_bind_group_layout, create_blit_pipeline, create_depth_texture, create_resolve_target,
        create_scene_color, draw_opaque_groups, draw_outline_groups,
        draw_transparent_groups_ordered, group_centers, transparent_order,
    },
    skinning::{compute_bone_matrices, Pose},
};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    camera_position: [f32; 4],
    /// 只有 toon.wgsl 读取（sphere map 需要在相机空间算反射）。
    view_rotation: [[f32; 4]; 4],
    /// 方向光颜色 × 强度；rgb 参与漫反射，a 预留。将来接 VMD 灯光帧时改这里。
    light: [f32; 4],
}

/// MMD 原生方向光的默认强度：diffuse 先乘它，再与 ambient 相加。
///
/// 不做这一项时 `diffuse(1.0) + ambient(0.5)` 会超过 1，亮面被 clamp 削平，
/// toon 阴影也失去可见度。
const MMD_DEFAULT_LIGHT: f32 = 0.6;

fn matrices_to_floats(matrices: &[Mat4]) -> Vec<f32> {
    matrices
        .iter()
        .flat_map(|matrix| matrix.to_cols_array())
        .collect()
}

/// 时间戳查询需要的缓冲区：一个接时间戳解析结果，一个给 CPU 映射读回。
///
/// 两者必须分开——`MAP_READ` 不能和 `COPY_DST` 混在同一个 buffer 上。
///
/// 只在适配器支持 `TIMESTAMP_QUERY` 时创建；不支持的设备上整条链路为 `None`，
/// 计时接口直接返回"不支持"，而不是给一个假数字。
struct GpuTimer {
    query_set: wgpu::QuerySet,
    /// 解析查询结果的目标，只给 GPU 用。
    resolve: wgpu::Buffer,
    /// 拷过来再映射，避免整个 resolve buffer 变成 CPU 可读。
    readback: wgpu::Buffer,
}

/// 渲染目标的尺寸与颜色格式。
///
/// 有窗口时这两项来自 surface，无头渲染时由调用方直接给定；其余所有与分辨率
/// 绑定的资源（深度、离屏颜色、blit 绑定）只读这一份。
#[derive(Clone, Copy, Debug)]
struct TargetInfo {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    /// 场景颜色目标的采样数；1 表示不做多重采样。
    samples: u32,
}

/// 平台无关的渲染器：持有设备上下文、管线与一帧的绘制计划，负责把画面呈现出去。
pub struct Renderer {
    /// 设备层：instance / adapter / device / queue 都从这里借。
    gpu: GpuContext,
    /// 有窗口时才有 surface。无头模式下画面只落在 `scene_color` 上。
    surface: Option<wgpu::Surface<'static>>,
    config: Option<wgpu::SurfaceConfiguration>,
    target: TargetInfo,
    pipelines: PipelineCache,
    /// 本帧的 pass 计划：顺序与每步的管线、绘制范围都在这里。
    scene_passes: ScenePasses,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    bone_buffer: wgpu::Buffer,
    bones: Vec<Bone>,
    animation: Option<AnimationClip>,
    material_bind_groups: Vec<wgpu::BindGroup>,
    material_transparent: Vec<bool>,
    material_outline: Vec<bool>,
    texture_slots: Vec<TextureSlot>,
    toon_slots: Vec<TextureSlot>,
    // 由 material bind group 持有，但 Rust 侧无需再次读取
    #[allow(dead_code)]
    sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    /// 场景颜色先画到这里，再由 blit pass 搬到 surface。
    scene_color: wgpu::Texture,
    scene_color_view: wgpu::TextureView,
    /// 开启 MSAA 时的多重采样渲染目标；单采样时为空，场景直接画进 `scene_color`。
    scene_msaa: Option<wgpu::Texture>,
    scene_msaa_view: Option<wgpu::TextureView>,
    blit_layout: wgpu::BindGroupLayout,
    blit_sampler: wgpu::Sampler,
    blit_pipeline: wgpu::RenderPipeline,
    blit_bind_group: wgpu::BindGroup,
    fps_overlay: Option<FpsOverlay>,
    /// 设备支持时间戳查询时才有值。
    gpu_timer: Option<GpuTimer>,
    camera: OrbitCamera,
    groups: Vec<crate::model::MaterialGroup>,
    group_centers: Vec<Vec3>,
    index_count: u32,
    model_name: String,
    start: Instant,
}

impl Renderer {
    /// 使用内置三角形创建渲染器。
    pub async fn new(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        size: (u32, u32),
        backends: wgpu::Backends,
    ) -> Result<Self> {
        Self::new_with_mesh(instance, surface, size, backends, Mesh::triangle()).await
    }

    /// 使用自定义网格创建渲染器（无贴图基目录）。
    pub async fn new_with_mesh(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        size: (u32, u32),
        backends: wgpu::Backends,
        mesh: Mesh,
    ) -> Result<Self> {
        Self::new_with_mesh_and_textures(instance, surface, size, backends, mesh, Path::new(""))
            .await
    }

    /// 使用自定义网格与贴图创建渲染器。
    pub async fn new_with_mesh_and_textures(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        size: (u32, u32),
        backends: wgpu::Backends,
        mesh: Mesh,
        texture_base_dir: &Path,
    ) -> Result<Self> {
        Self::build(
            instance,
            Some(surface),
            size,
            backends,
            mesh,
            texture_base_dir,
        )
        .await
    }

    /// 无头渲染器：不创建 surface，画面只落在离屏颜色目标里。
    ///
    /// 截图、画面回归测试都走这里——它们不需要窗口，也不需要交换链。
    pub async fn new_headless(
        instance: wgpu::Instance,
        size: (u32, u32),
        backends: wgpu::Backends,
        mesh: Mesh,
        texture_base_dir: &Path,
    ) -> Result<Self> {
        Self::build(instance, None, size, backends, mesh, texture_base_dir).await
    }

    async fn build(
        instance: wgpu::Instance,
        surface: Option<wgpu::Surface<'static>>,
        size: (u32, u32),
        backends: wgpu::Backends,
        mesh: Mesh,
        texture_base_dir: &Path,
    ) -> Result<Self> {
        if !mesh.indices.len().is_multiple_of(3) {
            return Err(anyhow!("mesh index count must be a multiple of 3"));
        }
        if mesh.indices.is_empty() {
            return Err(anyhow!("mesh has no indices"));
        }

        let width = size.0.max(1);
        let height = size.1.max(1);

        // 时间戳查询不是所有适配器都有；有才要，没有就走"不支持"分支。
        // 在编码器里写时间戳需要额外 feature，拿不到就只申请基础的查询能力。
        let gpu = GpuContext::request(instance, backends, surface.as_ref(), |adapter| {
            let available = adapter.features();
            let mut features = wgpu::Features::empty();
            if available.contains(wgpu::Features::TIMESTAMP_QUERY) {
                features |= wgpu::Features::TIMESTAMP_QUERY;
                if available.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS) {
                    features |= wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
                }
            }
            features
        })
        .await?;

        // build 里大量资源创建都要 device / queue / adapter，这里取成本地别名；
        // wgpu 的这些句柄内部是引用计数，clone 只是多一个句柄，不复制资源。
        let device = gpu.device().clone();
        let queue = gpu.queue().clone();
        let adapter = gpu.adapter().clone();

        let gpu_timer = if device.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        ) {
            let byte_size = 2 * std::mem::size_of::<u64>() as u64;
            Some(GpuTimer {
                query_set: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("frame timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: 2,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("timestamp resolve"),
                    size: byte_size,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("timestamp readback"),
                    size: byte_size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
            })
        } else {
            log::info!(
                "GPU frame timing unavailable: adapter lacks TIMESTAMP_QUERY or \
                 TIMESTAMP_QUERY_INSIDE_ENCODERS"
            );
            None
        };

        // 有 surface 时必须用它的格式（blit 要往它上面画）；无头时固定选 sRGB RGBA8，
        // 与桌面端 surface 的实际格式一致，截图才和窗口看到的一样。
        let (format, config) = match surface.as_ref() {
            Some(surface) => {
                let capabilities = surface.get_capabilities(&adapter);
                let format = capabilities
                    .formats
                    .iter()
                    .copied()
                    .find(|format| format.is_srgb())
                    .or_else(|| capabilities.formats.first().copied())
                    .ok_or_else(|| anyhow!("surface has no supported texture format"))?;
                let mut config = surface
                    .get_default_config(&adapter, width, height)
                    .ok_or_else(|| anyhow!("surface has no supported default configuration"))?;
                config.format = format;
                surface.configure(&device, &config);
                (format, Some(config))
            }
            None => (wgpu::TextureFormat::Rgba8UnormSrgb, None),
        };

        // 多重采样级别由格式特性决定；深度附件必须同采样数，跟不上就整体关掉。
        let color_features = adapter.get_texture_format_features(format);
        let depth_features =
            adapter.get_texture_format_features(crate::renderer_helpers::DEPTH_FORMAT);
        let mut samples = crate::renderer_helpers::choose_sample_count(color_features.flags);
        if !depth_features.flags.sample_count_supported(samples) {
            samples = 1;
        }
        log::info!("scene target: {format:?}, {samples}x samples");

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut identity_bones = compute_bone_matrices(&mesh.bones, &Pose::default());
        if identity_bones.is_empty() {
            identity_bones.push(Mat4::IDENTITY);
        }
        let bone_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bone matrices"),
            contents: bytemuck::cast_slice(&matrices_to_floats(&identity_bones)),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        let global_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh bind group"),
            layout: &global_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bone_buffer.as_entire_binding(),
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        // toon 图是明暗查找表，不是可平铺纹理：必须 Clamp，
        // 否则采样到 v = 1.0 附近会回卷到另一端，亮端白色串进暗部。
        let ramp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("toon ramp sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let mut texture_slots =
            texture::load_textures(&device, &queue, texture_base_dir, &mesh.texture_paths)?;
        let fallback_texture = texture::white_texture(&device, &queue, "white-fallback");
        let fallback_view = fallback_texture.create_view(&wgpu::TextureViewDescriptor::default());
        texture_slots.push(TextureSlot {
            label: "white-fallback".into(),
            texture: fallback_texture,
            view: fallback_view.clone(),
            alpha_max: 255,
            toon: Default::default(),
        });
        let toon_slots = texture::load_toon_textures(&device, &queue, texture_base_dir);

        let material_state = create_material_state(
            &device,
            &material_layout,
            &sampler,
            &ramp_sampler,
            &texture_slots,
            &toon_slots,
            &fallback_view,
            &mesh.materials,
        );
        let material_bind_groups = material_state.bind_groups;
        let material_transparent = material_state.transparent;
        let material_outline = material_state.outline;

        let group_centers = group_centers(&mesh.vertices, &mesh.indices, &mesh.groups);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&global_layout), Some(&material_layout)],
            ..Default::default()
        });

        let mut pipelines = PipelineCache::new(&device);
        let has_outline = material_outline.iter().any(|enabled| *enabled);
        let scene_passes = ScenePasses::build(
            &mut pipelines,
            &device,
            &pipeline_layout,
            format,
            has_outline,
            samples,
        );

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vertices"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let depth_texture = create_depth_texture(&device, width, height, samples);
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // 离屏颜色目标 + 搬运管线：渲染先落到自己的颜色纹理，再 blit 到 surface。
        // 开 MSAA 时这里是多重采样目标，场景画完后解析进 `scene_color`（单采样、可采样、
        // 可回读）；关闭时场景直接画进 `scene_color`。
        // 无头模式下这条链停在 `scene_color` 上，回读与截图都从它取数据。
        let scene_msaa = if samples > 1 {
            Some(create_scene_color(&device, width, height, format, samples))
        } else {
            None
        };
        let scene_msaa_view = scene_msaa
            .as_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let scene_color = match scene_msaa.is_some() {
            true => create_resolve_target(&device, width, height, format),
            false => create_scene_color(&device, width, height, format, 1),
        };
        let scene_color_view = scene_color.create_view(&wgpu::TextureViewDescriptor::default());
        let blit_layout = blit_bind_group_layout(&device);
        let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blit sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blit_pipeline = create_blit_pipeline(&device, &blit_layout, format);
        let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit bind group"),
            layout: &blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&scene_color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&blit_sampler),
                },
            ],
        });

        Ok(Self {
            gpu,
            surface,
            config,
            target: TargetInfo {
                width,
                height,
                format,
                samples,
            },
            pipelines,
            scene_passes,
            bind_group,
            uniform_buffer,
            bone_buffer,
            bones: mesh.bones,
            animation: None,
            material_bind_groups,
            material_transparent,
            material_outline,
            texture_slots,
            toon_slots,
            sampler,
            vertex_buffer,
            index_buffer,
            depth_texture,
            depth_view,
            scene_color,
            scene_color_view,
            scene_msaa,
            scene_msaa_view,
            blit_layout,
            blit_sampler,
            blit_pipeline,
            blit_bind_group,
            fps_overlay: None,
            gpu_timer,
            camera: {
                let mut camera = OrbitCamera::default();
                camera.set_viewport(width, height);
                camera
            },
            groups: mesh.groups,
            group_centers,
            index_count: mesh.indices.len() as u32,
            model_name: mesh.name,
            start: Instant::now(),
        })
    }

    /// 尺寸变化时重建所有与分辨率绑定的目标：深度、离屏颜色、blit 绑定。
    fn recreate_targets(&mut self, width: u32, height: u32) {
        let samples = self.target.samples;
        let format = self.target.format;
        self.target = TargetInfo {
            width,
            height,
            format,
            samples,
        };
        self.depth_texture = create_depth_texture(self.gpu.device(), width, height, samples);
        self.depth_view = self
            .depth_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.scene_msaa = if samples > 1 {
            Some(create_scene_color(
                self.gpu.device(),
                width,
                height,
                format,
                samples,
            ))
        } else {
            None
        };
        self.scene_msaa_view = self
            .scene_msaa
            .as_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.scene_color = match self.scene_msaa.is_some() {
            true => create_resolve_target(self.gpu.device(), width, height, format),
            false => create_scene_color(self.gpu.device(), width, height, format, 1),
        };
        self.scene_color_view = self
            .scene_color
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.blit_bind_group = self
            .gpu
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blit bind group"),
                layout: &self.blit_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.scene_color_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.blit_sampler),
                    },
                ],
            });
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        if self.target.width == size.0 && self.target.height == size.1 {
            return;
        }
        if let Some(config) = self.config.as_mut() {
            config.width = size.0;
            config.height = size.1;
        }
        if let (Some(surface), Some(config)) = (self.surface.as_ref(), self.config.as_ref()) {
            surface.configure(self.gpu.device(), config);
        }
        self.recreate_targets(size.0, size.1);
        self.camera.set_viewport(size.0, size.1);
        if let Some(overlay) = &mut self.fps_overlay {
            overlay.update(self.gpu.queue(), size, Instant::now());
        }
    }

    /// 刷新本帧的逐帧状态：FPS 文本、动画姿态、全局 uniforms。
    ///
    /// 有窗口与无头两条渲染路径共用这一段，它们只差最后往哪里提交。
    fn prepare_frame(&mut self) {
        let now = Instant::now();
        if let Some(overlay) = &mut self.fps_overlay {
            overlay.tick(now);
            overlay.update(
                self.gpu.queue(),
                (self.target.width, self.target.height),
                now,
            );
            if let Err(error) = overlay.prepare(self.gpu.device(), self.gpu.queue()) {
                log::warn!("{error:#}");
            }
        }

        let elapsed = self.start.elapsed().as_secs_f32();
        let pose = self
            .animation
            .as_ref()
            .map(|clip| clip.sample(elapsed * 60.0, &self.bones));
        if let Some(pose) = pose {
            self.set_pose(&pose);
        }
        let model = Mat4::IDENTITY;
        let mvp = self.camera.view_projection() * model;
        let eye = self.camera.eye();
        let view_rotation = Mat4::from_mat3(Mat3::from_mat4(self.camera.view()));
        let uniforms = Uniforms {
            mvp: mvp.to_cols_array_2d(),
            model: model.to_cols_array_2d(),
            camera_position: [eye.x, eye.y, eye.z, 1.0],
            view_rotation: view_rotation.to_cols_array_2d(),
            light: [MMD_DEFAULT_LIGHT, MMD_DEFAULT_LIGHT, MMD_DEFAULT_LIGHT, 1.0],
        };
        self.gpu
            .queue()
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// 把整帧场景画进离屏颜色目标（深度 prepass / 描边 / 不透明 / 透明 / FPS）。
    fn encode_scene(&self, encoder: &mut wgpu::CommandEncoder) {
        let transparent_order = transparent_order(
            &self.groups,
            &self.group_centers,
            &self.material_transparent,
            self.camera.eye(),
        );

        // 开 MSAA 时画进多重采样目标，同时指定解析目标：本 pass 结束时
        // 硬件把多个样本平均进 `scene_color`，后面的 blit 与回读只看它。
        let color_view = self
            .scene_msaa_view
            .as_ref()
            .unwrap_or(&self.scene_color_view);
        let resolve_target = if self.scene_msaa_view.is_some() {
            Some(&self.scene_color_view)
        } else {
            None
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mesh pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.02,
                        g: 0.02,
                        b: 0.06,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for scene_pass in self.scene_passes.iter() {
            log::trace!("scene pass: {}", scene_pass.label);
            pass.set_pipeline(self.pipelines.get(scene_pass.pipeline));
            pass.set_bind_group(0, &self.bind_group, &[]);
            match scene_pass.draw {
                PassDraw::OpaqueGroups => draw_opaque_groups(
                    &mut pass,
                    &self.groups,
                    &self.material_bind_groups,
                    &self.material_transparent,
                ),
                PassDraw::OutlineGroups => draw_outline_groups(
                    &mut pass,
                    &self.groups,
                    &self.material_bind_groups,
                    &self.material_outline,
                    &self.material_transparent,
                ),
                PassDraw::TransparentGroupsOrdered => draw_transparent_groups_ordered(
                    &mut pass,
                    &self.groups,
                    &self.material_bind_groups,
                    &transparent_order,
                ),
            }
        }

        if let Some(overlay) = &self.fps_overlay {
            if let Err(error) = overlay.render(&mut pass) {
                log::warn!("{error:#}");
            }
        }
    }

    /// 把离屏结果搬到给定视图。这一步目前是纯搬运，但颜色目标已经独立，
    /// 后处理 / 截图 / 缩略图都可以挂在这条链上。
    fn encode_blit(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &self.blit_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// 只渲染到离屏颜色目标，不碰 surface。
    ///
    /// 无头模式、截图与画面回归测试走这里；结果留在 `scene_color` 里，
    /// 由回读接口取走。
    pub fn render_offscreen(&mut self) -> Result<()> {
        self.prepare_frame();
        let mut encoder =
            self.gpu
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("offscreen frame encoder"),
                });
        self.encode_scene(&mut encoder);
        self.gpu.queue().submit(Some(encoder.finish()));
        Ok(())
    }

    pub fn render_frame(&mut self) -> Result<()> {
        // 无 surface 时退化为无头渲染，调用方不必区分两种模式。
        if self.surface.is_none() {
            return self.render_offscreen();
        }
        self.prepare_frame();

        let acquired = self
            .surface
            .as_ref()
            .expect("surface checked above")
            .get_current_texture();
        let frame = match acquired {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                if let (Some(surface), Some(config)) = (self.surface.as_ref(), self.config.as_ref())
                {
                    surface.configure(self.gpu.device(), config);
                }
                let (width, height) = (self.target.width, self.target.height);
                self.recreate_targets(width, height);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(anyhow!("Vulkan surface lost; recreate the window/surface"));
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(anyhow!("surface validation error"));
            }
        };

        let view_texture = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.gpu
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frame encoder"),
                });
        self.encode_scene(&mut encoder);
        self.encode_blit(&mut encoder, &view_texture);

        self.gpu.queue().submit(Some(encoder.finish()));
        self.gpu.queue().present(frame);
        Ok(())
    }

    pub fn backend_label() -> &'static str {
        BACKEND_LABEL
    }

    /// 把离屏颜色目标读回 CPU，得到一帧 RGBA8 像素。
    ///
    /// 会阻塞等待 GPU 完成拷贝，只用于截图与测试，不要放进每帧循环。
    pub fn read_scene_rgba(&self) -> Result<FrameImage> {
        let width = self.target.width;
        let height = self.target.height;
        let unpadded_bytes_per_row = width * 4;
        // copy_texture_to_buffer 要求每行按 256 字节对齐，而画面宽度通常不是 64 的倍数，
        // 所以申请时按对齐后的行长算，读回后再把填充截掉。
        let padded_bytes_per_row = unpadded_bytes_per_row
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

        let readback = self.gpu.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene readback"),
            size: (padded_bytes_per_row as u64) * (height as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder =
            self.gpu
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("scene readback encoder"),
                });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.scene_color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue().submit(Some(encoder.finish()));

        let (sender, receiver) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.gpu
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| anyhow!("waiting for the scene readback failed: {error:?}"))?;
        receiver
            .recv()
            .map_err(|_| anyhow!("scene readback callback was dropped"))?
            .map_err(|error| anyhow!("scene readback mapping failed: {error:?}"))?;

        // surface 常见格式是 BGRA，纹理内存里也就是 BGRA；对外一律给 RGBA。
        let bgra = matches!(
            self.target.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
        {
            let view = readback
                .slice(..)
                .get_mapped_range()
                .map_err(|error| anyhow!("failed to map the readback buffer: {error:?}"))?;
            for row in 0..height as usize {
                let start = row * padded_bytes_per_row as usize;
                let end = start + unpadded_bytes_per_row as usize;
                let source = &view[start..end];
                if bgra {
                    for pixel in source.as_chunks::<4>().0 {
                        pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                    }
                } else {
                    pixels.extend_from_slice(source);
                }
            }
        }
        readback.unmap();

        Ok(FrameImage::new(width, height, pixels))
    }

    /// 渲染一帧到离屏目标并写成 PNG。
    ///
    /// 走的是无头路径，不依赖窗口；输出与窗口里看到的一致（1:1 blit）。
    pub fn capture_png(&mut self, path: &Path) -> Result<()> {
        self.render_offscreen()?;
        let image = self.read_scene_rgba()?;
        image.save_png(path)
    }

    /// 测量一帧场景在 GPU 上的耗时。
    ///
    /// 现场渲染一帧并阻塞等待时间戳读回，属于按需测量，不要放进每帧循环。
    /// 返回 `Ok(None)` 表示这台设备的适配器不支持时间戳查询。
    pub fn measure_gpu_time(&mut self) -> Result<Option<std::time::Duration>> {
        if self.gpu_timer.is_none() {
            return Ok(None);
        }

        self.prepare_frame();
        let timer = self.gpu_timer.as_ref().expect("gpu timer checked above");
        let mut encoder =
            self.gpu
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("gpu timer encoder"),
                });
        // 用编码器级时间戳把整段场景包起来：不依赖某个 render pass 的首尾，
        // 后面加 pass 或换帧结构都不用改这里。
        encoder.write_timestamp(&timer.query_set, 0);
        self.encode_scene(&mut encoder);
        encoder.write_timestamp(&timer.query_set, 1);
        encoder.resolve_query_set(&timer.query_set, 0..2, &timer.resolve, 0);
        encoder.copy_buffer_to_buffer(&timer.resolve, 0, &timer.readback, 0, 16);
        self.gpu.queue().submit(Some(encoder.finish()));

        let (sender, receiver) = std::sync::mpsc::channel();
        timer
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.gpu
            .device()
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| anyhow!("waiting for the timestamps failed: {error:?}"))?;
        receiver
            .recv()
            .map_err(|_| anyhow!("timestamp readback callback was dropped"))?
            .map_err(|error| anyhow!("timestamp readback mapping failed: {error:?}"))?;

        let (begin, end) = {
            let view = timer
                .readback
                .slice(..)
                .get_mapped_range()
                .map_err(|error| anyhow!("failed to map the timestamp buffer: {error:?}"))?;
            let bytes: &[u8] = &view;
            let begin = u64::from_le_bytes(bytes[0..8].try_into().expect("8 bytes"));
            let end = u64::from_le_bytes(bytes[8..16].try_into().expect("8 bytes"));
            (begin, end)
        };
        timer.readback.unmap();

        // 有些驱动声称支持时间戳，实际写出来全是 0（本机的 RX 5700 XT 就是这样）。
        // 这种情况按"没有可用时钟"处理，绝不返回一个看起来正常的 0 ms。
        if begin == 0 && end == 0 {
            log::warn!("GPU timestamps read back as zero; the driver exposes no usable clock");
            return Ok(None);
        }

        // 时间戳是设备时钟的滴答数，乘上每滴答多少纳秒才是真实时间。
        let period_ns = f64::from(self.gpu.queue().get_timestamp_period());
        let elapsed_ns = end.saturating_sub(begin) as f64 * period_ns;
        Ok(Some(std::time::Duration::from_secs_f64(
            elapsed_ns / 1_000_000_000.0,
        )))
    }

    pub fn adapter_name(&self) -> &str {
        self.gpu.adapter_name()
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    pub fn triangle_count(&self) -> u32 {
        self.index_count / 3
    }

    pub fn texture_count(&self) -> usize {
        self.texture_slots.len().saturating_sub(1)
    }

    pub fn toon_texture_count(&self) -> usize {
        self.toon_slots.len()
    }

    pub fn bone_count(&self) -> usize {
        self.bones.len()
    }

    pub fn set_animation(&mut self, clip: AnimationClip) {
        self.animation = Some(clip);
    }

    /// 开启左上角 FPS 叠加；`font_path` 可指定 OpenType 字体文件。
    /// 返回 false 表示没有可用字体，此时不绘制文字。
    pub fn enable_fps_overlay(&mut self, font_path: Option<&Path>) -> bool {
        let overlay = FpsOverlay::new(
            self.gpu.device(),
            self.gpu.queue(),
            self.target.format,
            (self.target.width, self.target.height),
            self.target.samples,
            font_path,
        );
        let enabled = overlay.enabled();
        self.fps_overlay = Some(overlay);
        enabled
    }

    pub fn fps(&self) -> Option<f32> {
        self.fps_overlay.as_ref().and_then(FpsOverlay::fps)
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.camera.orbit(dx, dy);
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.camera.pan(dx, dy);
    }

    pub fn zoom(&mut self, notches: f32) {
        self.camera.zoom(notches);
    }

    pub fn reset_camera(&mut self) {
        self.camera.reset();
    }

    pub fn camera(&self) -> &OrbitCamera {
        &self.camera
    }

    pub fn set_pose(&mut self, pose: &Pose) {
        let mut matrices = compute_bone_matrices(&self.bones, pose);
        if matrices.is_empty() {
            matrices.push(Mat4::IDENTITY);
        }
        self.gpu.queue().write_buffer(
            &self.bone_buffer,
            0,
            bytemuck::cast_slice(&matrices_to_floats(&matrices)),
        );
    }

    pub fn instance(&self) -> &wgpu::Instance {
        self.gpu.instance()
    }
}
