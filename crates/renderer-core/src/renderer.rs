use std::{borrow::Cow, path::Path, time::Instant};

use anyhow::{anyhow, Context, Result};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::{
    animation::VmdClip,
    backend::BACKEND_LABEL,
    model::{
        texture::{self, TextureSlot},
        Bone, Mesh, MeshVertex,
    },
    overlay::FpsOverlay,
    skinning::{compute_bone_matrices, Pose},
};

const SHADER_SOURCE: &str = include_str!("shaders/toon.wgsl");
const OUTLINE_SOURCE: &str = include_str!("shaders/outline.wgsl");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MaterialUniform {
    diffuse: [f32; 4],
    edge_color: [f32; 4],
    params: [f32; 4],
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
    outline_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    bone_buffer: wgpu::Buffer,
    bones: Vec<Bone>,
    animation: Option<VmdClip>,
    material_bind_groups: Vec<wgpu::BindGroup>,
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
    groups: Vec<crate::model::MaterialGroup>,
    index_count: u32,
    model_name: String,
    start: Instant,
}

fn create_depth_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
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
            view: fallback_view,
        });
        let toon_slots = texture::internal_toon_textures(&device, &queue);

        let mut material_bind_groups = Vec::with_capacity(mesh.materials.len());
        for material in &mesh.materials {
            let uniform = MaterialUniform {
                diffuse: material.diffuse,
                edge_color: material.edge_color,
                params: [material.edge_scale, material.sphere_mode as f32, 0.0, 0.0],
            };
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("material uniform"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let view = material
                .texture_index
                .and_then(|index| texture_slots.get(index))
                .map(|slot| slot.view.clone())
                .unwrap_or_else(|| texture_slots.last().unwrap().view.clone());
            let toon_view = if material.toon_reference == 0 {
                usize::try_from(material.toon_index)
                    .ok()
                    .and_then(|index| texture_slots.get(index))
                    .or_else(|| toon_slots.first())
            } else {
                usize::try_from(material.toon_index)
                    .ok()
                    .and_then(|index| toon_slots.get(index))
                    .or_else(|| toon_slots.first())
            }
            .map(|slot| slot.view.clone())
            .unwrap_or_else(|| texture_slots.last().unwrap().view.clone());
            let sphere_view = material
                .sphere_index
                .and_then(|index| texture_slots.get(index))
                .map(|slot| slot.view.clone())
                .unwrap_or_else(|| texture_slots.last().unwrap().view.clone());
            material_bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("material {}", material.name)),
                layout: &material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&toon_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&sphere_view),
                    },
                ],
            }));
        }

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
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 24,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 36,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32x4,
                            offset: 44,
                            shader_location: 4,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 60,
                            shader_location: 5,
                        },
                    ],
                })],
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
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
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
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 24,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 36,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32x4,
                            offset: 44,
                            shader_location: 4,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 60,
                            shader_location: 5,
                        },
                    ],
                })],
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
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
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
            outline_pipeline,
            bind_group,
            uniform_buffer,
            bone_buffer,
            bones: mesh.bones,
            animation: None,
            material_bind_groups,
            texture_slots,
            toon_slots,
            sampler,
            vertex_buffer,
            index_buffer,
            depth_texture,
            depth_view,
            fps_overlay: None,
            groups: mesh.groups,
            index_count: mesh.indices.len() as u32,
            model_name: mesh.name,
            start: Instant::now(),
        })
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
        self.depth_texture = create_depth_texture(&self.device, size.0, size.1);
        self.depth_view = self
            .depth_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
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
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let model = Mat4::from_rotation_y(elapsed * 0.9) * Mat4::from_rotation_x(elapsed * 0.35);
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 3.4), Vec3::ZERO, Vec3::Y);
        let projection = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, aspect, 0.1, 100.0);
        let mvp = projection * view * model;
        let uniforms = Uniforms {
            mvp: mvp.to_cols_array_2d(),
            model: model.to_cols_array_2d(),
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                self.depth_texture =
                    create_depth_texture(&self.device, self.config.width, self.config.height);
                self.depth_view = self
                    .depth_texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
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
            pass.set_pipeline(&self.outline_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for group in &self.groups {
                if group.index_count == 0 {
                    continue;
                }
                if let Some(bind_group) = self.material_bind_groups.get(group.material_index) {
                    pass.set_bind_group(1, bind_group, &[]);
                }
                pass.draw_indexed(
                    group.index_start as u32..(group.index_start + group.index_count) as u32,
                    0,
                    0..1,
                );
            }

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for group in &self.groups {
                if group.index_count == 0 {
                    continue;
                }
                if let Some(bind_group) = self.material_bind_groups.get(group.material_index) {
                    pass.set_bind_group(1, bind_group, &[]);
                }
                pass.draw_indexed(
                    group.index_start as u32..(group.index_start + group.index_count) as u32,
                    0,
                    0..1,
                );
            }

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
