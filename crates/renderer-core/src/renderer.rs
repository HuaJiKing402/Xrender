use std::{borrow::Cow, path::Path, time::Instant};

use anyhow::{anyhow, Context, Result};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::{
    animation::VmdClip,
    backend::BACKEND_LABEL,
    camera::OrbitCamera,
    material::create_material_state,
    model::{
        texture::{self, TextureSlot},
        Bone, Mesh,
    },
    overlay::FpsOverlay,
    renderer_helpers::{
        create_depth_texture, depth_state, depth_state_with, draw_opaque_groups,
        draw_transparent_groups_ordered, group_centers, transparent_order, vertex_layout,
    },
    skinning::{compute_bone_matrices, Pose},
};

const SHADER_SOURCE: &str = include_str!("shaders/toon.wgsl");
const OUTLINE_SOURCE: &str = include_str!("shaders/outline.wgsl");
const DEPTH_SOURCE: &str = include_str!("shaders/depth.wgsl");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
}

fn matrices_to_floats(matrices: &[Mat4]) -> Vec<f32> {
    matrices
        .iter()
        .flat_map(|matrix| matrix.to_cols_array())
        .collect()
}

/// 平台无关的渲染器：接收一个已经创建好的 surface，负责设备、管线与帧呈现。
pub struct Renderer {
    instance: wgpu::Instance,
    adapter_name: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    transparent_pipeline: wgpu::RenderPipeline,
    depth_pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    bone_buffer: wgpu::Buffer,
    bones: Vec<Bone>,
    animation: Option<VmdClip>,
    material_bind_groups: Vec<wgpu::BindGroup>,
    material_transparent: Vec<bool>,
    texture_slots: Vec<TextureSlot>,
    toon_slots: Vec<TextureSlot>,
    // 由 material bind group 持有，但 Rust 侧无需再次读取
    #[allow(dead_code)]
    sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    fps_overlay: Option<FpsOverlay>,
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
        if !mesh.indices.len().is_multiple_of(3) {
            return Err(anyhow!("mesh index count must be a multiple of 3"));
        }
        if mesh.indices.is_empty() {
            return Err(anyhow!("mesh has no indices"));
        }

        let adapter_count = instance.enumerate_adapters(backends).await.len();
        if adapter_count == 0 {
            return Err(anyhow!(
                "no {} adapter found. Install or update your GPU driver.",
                crate::backend::label(backends)
            ));
        }
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .context("failed to request a compatible GPU adapter")?;

        let adapter_name = adapter.get_info().name;
        log::info!("adapter: {adapter_name}");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .context("failed to request GPU device")?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| anyhow!("surface has no supported texture format"))?;

        let mut config = surface
            .get_default_config(&adapter, size.0.max(1), size.1.max(1))
            .ok_or_else(|| anyhow!("surface has no supported default configuration"))?;
        config.format = format;
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("toon.wgsl"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER_SOURCE)),
        });
        let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("outline.wgsl"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(OUTLINE_SOURCE)),
        });
        let depth_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth.wgsl"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(DEPTH_SOURCE)),
        });

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
        });
        let toon_slots = texture::internal_toon_textures(&device, &queue);

        let material_state = create_material_state(
            &device,
            &material_layout,
            &sampler,
            &texture_slots,
            &toon_slots,
            &fallback_view,
            &mesh.materials,
        );
        let material_bind_groups = material_state.bind_groups;
        let material_transparent = material_state.transparent;

        let group_centers = group_centers(&mesh.vertices, &mesh.indices, &mesh.groups);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&global_layout), Some(&material_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(vertex_layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_state_with(false, wgpu::CompareFunction::LessEqual)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("depth prepass pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &depth_shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(vertex_layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &depth_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_state(true)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let transparent_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("transparent mesh pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(vertex_layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth_state_with(false, wgpu::CompareFunction::LessEqual)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let outline_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("outline pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &outline_shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(vertex_layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &outline_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Front),
                ..Default::default()
            },
            depth_stencil: Some(depth_state_with(false, wgpu::CompareFunction::LessEqual)),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

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
        let depth_texture = create_depth_texture(&device, size.0, size.1);
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        Ok(Self {
            instance,
            adapter_name,
            device,
            queue,
            surface,
            config,
            pipeline,
            transparent_pipeline,
            depth_pipeline,
            outline_pipeline,
            bind_group,
            uniform_buffer,
            bone_buffer,
            bones: mesh.bones,
            animation: None,
            material_bind_groups,
            material_transparent,
            texture_slots,
            toon_slots,
            sampler,
            vertex_buffer,
            index_buffer,
            depth_texture,
            depth_view,
            fps_overlay: None,
            camera: {
                let mut camera = OrbitCamera::default();
                camera.set_viewport(size.0, size.1);
                camera
            },
            groups: mesh.groups,
            group_centers,
            index_count: mesh.indices.len() as u32,
            model_name: mesh.name,
            start: Instant::now(),
        })
    }

    fn recreate_depth(&mut self, width: u32, height: u32) {
        self.depth_texture = create_depth_texture(&self.device, width, height);
        self.depth_view = self
            .depth_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        if self.config.width == size.0 && self.config.height == size.1 {
            return;
        }
        self.config.width = size.0;
        self.config.height = size.1;
        self.surface.configure(&self.device, &self.config);
        self.recreate_depth(size.0, size.1);
        self.camera.set_viewport(size.0, size.1);
        if let Some(overlay) = &mut self.fps_overlay {
            overlay.update(&self.queue, size, Instant::now());
        }
    }

    pub fn render_frame(&mut self) -> Result<()> {
        let now = Instant::now();
        if let Some(overlay) = &mut self.fps_overlay {
            overlay.tick(now);
            overlay.update(&self.queue, (self.config.width, self.config.height), now);
            if let Err(error) = overlay.prepare(&self.device, &self.queue) {
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
        let uniforms = Uniforms {
            mvp: mvp.to_cols_array_2d(),
            model: model.to_cols_array_2d(),
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let transparent_order = transparent_order(
            &self.groups,
            &self.group_centers,
            &self.material_transparent,
            self.camera.eye(),
        );

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                let (width, height) = (self.config.width, self.config.height);
                self.recreate_depth(width, height);
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
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view_texture,
                    depth_slice: None,
                    resolve_target: None,
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
            pass.set_pipeline(&self.depth_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            draw_opaque_groups(
                &mut pass,
                &self.groups,
                &self.material_bind_groups,
                &self.material_transparent,
            );

            pass.set_pipeline(&self.outline_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            draw_opaque_groups(
                &mut pass,
                &self.groups,
                &self.material_bind_groups,
                &self.material_transparent,
            );

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            draw_opaque_groups(
                &mut pass,
                &self.groups,
                &self.material_bind_groups,
                &self.material_transparent,
            );

            pass.set_pipeline(&self.transparent_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            draw_transparent_groups_ordered(
                &mut pass,
                &self.groups,
                &self.material_bind_groups,
                &transparent_order,
            );

            if let Some(overlay) = &self.fps_overlay {
                if let Err(error) = overlay.render(&mut pass) {
                    log::warn!("{error:#}");
                }
            }
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        Ok(())
    }

    pub fn backend_label() -> &'static str {
        BACKEND_LABEL
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
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

    pub fn set_animation(&mut self, clip: VmdClip) {
        self.animation = Some(clip);
    }

    /// 开启左上角 FPS 叠加；`font_path` 可指定 OpenType 字体文件。
    /// 返回 false 表示没有可用字体，此时不绘制文字。
    pub fn enable_fps_overlay(&mut self, font_path: Option<&Path>) -> bool {
        let overlay = FpsOverlay::new(
            &self.device,
            &self.queue,
            self.config.format,
            (self.config.width, self.config.height),
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
        self.queue.write_buffer(
            &self.bone_buffer,
            0,
            bytemuck::cast_slice(&matrices_to_floats(&matrices)),
        );
    }

    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }
}
