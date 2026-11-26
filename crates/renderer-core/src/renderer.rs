use std::{path::Path, time::Instant};

use anyhow::{anyhow, Result};
use glam::{Mat3, Mat4};

use crate::{
    animation::AnimationClip,
    backend::BACKEND_LABEL,
    camera::OrbitCamera,
    capture::FrameImage,
    gpu::GpuContext,
    model::Mesh,
    overlay::FpsOverlay,
    pass::{PassDraw, ScenePasses},
    pipeline::PipelineCache,
    renderer_helpers::{
        blit_bind_group_layout, create_blit_pipeline, create_depth_texture, create_resolve_target,
        create_scene_color,
    },
    scene::{transparent_draw_order, validate_mesh, FrameUniforms, GpuMesh, Instance},
    skinning::Pose,
};

/// MMD 原生方向光的默认强度：diffuse 先乘它，再与 ambient 相加。
///
/// 不做这一项时 `diffuse(1.0) + ambient(0.5)` 会超过 1，亮面被 clamp 削平，
/// toon 阴影也失去可见度。
const MMD_DEFAULT_LIGHT: f32 = 0.6;

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
    /// 管线布局：group 0 每对象、group 1 材质；加新网格要重建 pass 计划时用得上。
    pipeline_layout: wgpu::PipelineLayout,
    /// group 0 的布局：每对象 uniform + 骨骼矩阵。
    object_layout: wgpu::BindGroupLayout,
    /// group 1 的布局：材质 uniform 与贴图。
    material_layout: wgpu::BindGroupLayout,
    /// 场景里的网格资源；实例按下标引用，同模多实例只上传一次几何。
    meshes: Vec<GpuMesh>,
    /// 场景里的实例；不透明分组按这个顺序画，透明分组另按距离排序。
    instances: Vec<Instance>,
    sampler: wgpu::Sampler,
    ramp_sampler: wgpu::Sampler,
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
        validate_mesh(&mesh)?;

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

        // 绑定布局与采样器是全场景共用的；网格资源与实例各自持有自己的 GPU 数据。
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object bind group layout"),
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

        // 第一份资源 + 它的第一个实例（单位变换）。
        // "同模多实例"与"多网格"都是在这之后往 `meshes` / `instances` 里追加。
        let first_mesh = GpuMesh::upload(
            &device,
            &queue,
            mesh,
            texture_base_dir,
            &material_layout,
            &sampler,
            &ramp_sampler,
        )?;
        let has_outline = first_mesh.has_outline();
        let first_instance = Instance::new(&device, &object_layout, 0, &first_mesh, Mat4::IDENTITY);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&object_layout), Some(&material_layout)],
            ..Default::default()
        });

        let mut pipelines = PipelineCache::new(&device);
        let scene_passes = ScenePasses::build(
            &mut pipelines,
            &device,
            &pipeline_layout,
            format,
            has_outline,
            samples,
        );

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
            pipeline_layout,
            object_layout,
            material_layout,
            meshes: vec![first_mesh],
            instances: vec![first_instance],
            sampler,
            ramp_sampler,
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
            start: Instant::now(),
        })
    }

    /// 往场景里加一份网格资源，并为它建第一个实例。
    ///
    /// 返回新实例的编号。同一份资源可以再加实例（[`Self::add_instance`]），
    /// 几何只上传一次。
    ///
    /// 若新网格带描边而当前 pass 计划里没有深度预通道，会重建 pass 计划
    /// （管线都在缓存里，重建只是重新组装顺序，不会重复编译）。
    pub fn add_mesh(
        &mut self,
        mesh: Mesh,
        texture_base_dir: &Path,
        transform: Mat4,
    ) -> Result<usize> {
        validate_mesh(&mesh)?;
        let device = self.gpu.device();
        let queue = self.gpu.queue();
        let uploaded = GpuMesh::upload(
            device,
            queue,
            mesh,
            texture_base_dir,
            &self.material_layout,
            &self.sampler,
            &self.ramp_sampler,
        )?;

        let mesh_index = self.meshes.len();
        let instance = Instance::new(
            device,
            &self.object_layout,
            mesh_index,
            &uploaded,
            transform,
        );
        let has_outline = uploaded.has_outline();
        self.meshes.push(uploaded);
        self.instances.push(instance);
        if has_outline {
            self.ensure_scene_passes();
        }
        Ok(self.instances.len() - 1)
    }

    /// 复制一份已有的资源，挂上独立变换——同模多实例不重复上传几何。
    pub fn add_instance(&mut self, mesh: usize, transform: Mat4) -> Result<usize> {
        let Some(resource) = self.meshes.get(mesh) else {
            return Err(anyhow!("no mesh with index {mesh}"));
        };
        let device = self.gpu.device();
        let instance = Instance::new(device, &self.object_layout, mesh, resource, transform);
        self.instances.push(instance);
        Ok(self.instances.len() - 1)
    }

    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// 改一个实例的变换；下一帧生效。
    pub fn set_instance_transform(&mut self, instance: usize, transform: Mat4) -> Result<()> {
        let Some(target) = self.instances.get_mut(instance) else {
            return Err(anyhow!("no instance with index {instance}"));
        };
        target.set_transform(transform);
        Ok(())
    }

    /// 给单个实例挂动画（`None` 表示回到静止姿态）；同模的另一实例不受影响。
    pub fn set_instance_animation(
        &mut self,
        instance: usize,
        clip: Option<AnimationClip>,
    ) -> Result<()> {
        let Some(target) = self.instances.get_mut(instance) else {
            return Err(anyhow!("no instance with index {instance}"));
        };
        target.set_animation(clip);
        Ok(())
    }

    /// 有描边材质时必须存在深度预通道；场景内容变了就按新内容重建 pass 计划。
    fn ensure_scene_passes(&mut self) {
        let has_outline = self.meshes.iter().any(GpuMesh::has_outline);
        let expects_outline = self
            .scene_passes
            .iter()
            .any(|pass| pass.label == "depth-prepass");
        if has_outline == expects_outline {
            return;
        }
        self.scene_passes = ScenePasses::build(
            &mut self.pipelines,
            self.gpu.device(),
            &self.pipeline_layout,
            self.target.format,
            has_outline,
            self.target.samples,
        );
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

    /// 刷新本帧的逐帧状态：FPS 文本、各实例的动画姿态与每对象 uniforms。
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
        let eye = self.camera.eye();
        let view_projection = self.camera.view_projection();
        let frame = FrameUniforms {
            camera_position: [eye.x, eye.y, eye.z, 1.0],
            view_rotation: Mat4::from_mat3(Mat3::from_mat4(self.camera.view())).to_cols_array_2d(),
            light: [MMD_DEFAULT_LIGHT, MMD_DEFAULT_LIGHT, MMD_DEFAULT_LIGHT, 1.0],
        };

        let queue = self.gpu.queue();
        let meshes = &self.meshes;
        for instance in &mut self.instances {
            let Some(resource) = meshes.get(instance.mesh()) else {
                continue;
            };
            // 只有绑定了动画的实例才重算骨骼；其余保持静止姿态，省掉一次全量变换。
            let pose = instance
                .animation()
                .map(|clip| clip.sample(elapsed * 60.0, resource.bones()));
            if let Some(pose) = pose {
                instance.apply_pose(queue, resource.bones(), &pose);
            }
            instance.write_uniforms(queue, &frame, view_projection);
        }
    }

    /// 把整帧场景画进离屏颜色目标（深度 prepass / 描边 / 不透明 / 透明 / FPS）。
    fn encode_scene(&self, encoder: &mut wgpu::CommandEncoder) {
        // 透明分组跨实例统一排序：一个实例的近处透明面必须排在另一个实例的远处透明面之后。
        let transparent_draws =
            transparent_draw_order(&self.meshes, &self.instances, self.camera.eye());

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
        for scene_pass in self.scene_passes.iter() {
            log::trace!("scene pass: {}", scene_pass.label);
            pass.set_pipeline(self.pipelines.get(scene_pass.pipeline));
            match scene_pass.draw {
                PassDraw::TransparentGroupsOrdered => {
                    // 顺序是全局的，所以按 (实例, 分组) 直接画，不按实例分组循环。
                    for &(instance_index, group_index) in &transparent_draws {
                        let Some(instance) = self.instances.get(instance_index) else {
                            continue;
                        };
                        let Some(mesh) = self.meshes.get(instance.mesh()) else {
                            continue;
                        };
                        pass.set_bind_group(0, instance.bind_group(), &[]);
                        mesh.bind(&mut pass);
                        mesh.draw_transparent(&mut pass, &[group_index]);
                    }
                }
                draw_kind => {
                    for instance in &self.instances {
                        let Some(mesh) = self.meshes.get(instance.mesh()) else {
                            continue;
                        };
                        pass.set_bind_group(0, instance.bind_group(), &[]);
                        mesh.bind(&mut pass);
                        match draw_kind {
                            PassDraw::OpaqueGroups => mesh.draw_opaque(&mut pass),
                            PassDraw::OutlineGroups => mesh.draw_outline(&mut pass),
                            PassDraw::TransparentGroupsOrdered => unreachable!("handled above"),
                        }
                    }
                }
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

    /// 第一份网格资源名；多网格场景下用 [`Self::mesh_name`]。
    pub fn model_name(&self) -> &str {
        self.meshes
            .first()
            .map(GpuMesh::name)
            .unwrap_or("(no mesh)")
    }

    pub fn mesh_name(&self, mesh: usize) -> Option<&str> {
        self.meshes.get(mesh).map(GpuMesh::name)
    }

    /// 场景里所有网格资源的三角形总数。
    pub fn triangle_count(&self) -> u32 {
        self.meshes.iter().map(GpuMesh::triangle_count).sum()
    }

    /// 全部资源用到的不重复贴图数（同一张图被两份网格引用只算一次，
    /// 但这里按"每个资源各算一份"计——资源之间本来就不共享贴图表）。
    pub fn texture_count(&self) -> usize {
        self.meshes
            .iter()
            .map(|mesh| mesh.textures().diffuse().len())
            .sum()
    }

    pub fn toon_texture_count(&self) -> usize {
        self.meshes
            .first()
            .map(|mesh| mesh.textures().ramps().len())
            .unwrap_or(0)
    }

    /// 第一份资源的骨骼数；骨骼是每个资源一份的。
    pub fn bone_count(&self) -> usize {
        self.meshes.first().map(GpuMesh::bone_count).unwrap_or(0)
    }

    /// 给第一个实例挂动画，兼容单模型场景的旧调用方式。
    pub fn set_animation(&mut self, clip: AnimationClip) {
        if let Some(instance) = self.instances.first_mut() {
            instance.set_animation(Some(clip));
        }
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

    /// 手动把一个姿态套到第一个实例上（动画之外的入口）。
    ///
    /// 想控制别的实例用 [`Self::set_instance_animation`] 或直接给该实例建动画。
    pub fn set_pose(&mut self, pose: &Pose) {
        let Some(instance) = self.instances.first() else {
            return;
        };
        let Some(resource) = self.meshes.get(instance.mesh()) else {
            return;
        };
        instance.apply_pose(self.gpu.queue(), resource.bones(), pose);
    }

    pub fn instance(&self) -> &wgpu::Instance {
        self.gpu.instance()
    }
}
