use std::{borrow::Cow, time::Instant};

use anyhow::{anyhow, Context, Result};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::{
    backend::BACKEND_LABEL,
    model::{Mesh, MeshVertex},
};

const SHADER_SOURCE: &str = include_str!("shaders/triangles.wgsl");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
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
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
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

    /// 使用自定义网格创建渲染器。
    pub async fn new_with_mesh(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        size: (u32, u32),
        backends: wgpu::Backends,
        mesh: Mesh,
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
            label: Some("triangles.wgsl"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER_SOURCE)),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
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
            depth_stencil: None,
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

        Ok(Self {
            instance,
            adapter_name,
            device,
            queue,
            surface,
            config,
            pipeline,
            bind_group,
            uniform_buffer,
            vertex_buffer,
            index_buffer,
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
    }

    pub fn render_frame(&mut self) -> Result<()> {
        let elapsed = self.start.elapsed().as_secs_f32();
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
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
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

    pub fn instance(&self) -> &wgpu::Instance {
        &self.instance
    }
}
